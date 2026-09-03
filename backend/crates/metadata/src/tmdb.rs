//! The Movie Database (TMDB) provider — the default (`docs/.tasks/60` §Sub-tasks 2).
//!
//! Talks to `api.themoviedb.org/3`: `/search/movie` + `/search/tv` for candidates,
//! `/movie/{id}?append_to_response=credits` (and the TV equivalent) for details, and
//! `/configuration` once per process for the image base URL. A [`tokio::sync::Semaphore`]
//! bounds concurrent requests so a first-run scan of a large library respects TMDB's
//! rate limits.
//!
//! JSON parsing is split out into pure functions ([`parse_search`], [`parse_details`])
//! that take the raw `serde_json::Value` so the crate's tests can exercise scoring and
//! field extraction against **recorded fixtures** with no live network.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::{OnceCell, Semaphore};

use crate::matcher;
use crate::provider::{
    Collection, CreditIn, Details, Genre, Match, MediaKind, MetadataProvider, PersonDetails,
    ProviderId, TrailerIn,
};
use crate::{Error, Result};

const API_BASE: &str = "https://api.themoviedb.org/3";
/// A safe fallback image base if `/configuration` cannot be reached. TMDB's canonical
/// CDN host; `w780` is a reasonable poster/backdrop width for a 10-foot UI.
const FALLBACK_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/w780";
/// A safe fallback profile-image base for headshots (`docs/.tasks/91` Phase B). `h632` is
/// TMDB's tallest fixed profile width — a good size for a person-page portrait.
const FALLBACK_PROFILE_BASE: &str = "https://image.tmdb.org/t/p/h632";
/// Max concurrent TMDB requests. TMDB's documented limit is generous (~50/s) but we stay
/// well under it during a burst scan; the enrichment worker also bounds its own fan-out.
const MAX_CONCURRENCY: usize = 8;

/// The default TMDB provider.
pub struct TmdbProvider {
    api_key: String,
    language: String,
    http: reqwest::Client,
    sem: Arc<Semaphore>,
    /// The image base URL from `/configuration`, fetched once and reused (a courtesy
    /// cache to avoid a round-trip per title — `docs/.tasks/60` §Provider-response cache).
    image_base: OnceCell<String>,
    /// The profile-image base URL for headshots (`docs/.tasks/91` Phase B) — a second
    /// cached width from the same `/configuration` response, resolved lazily like
    /// [`Self::image_base`].
    profile_base: OnceCell<String>,
}

impl TmdbProvider {
    /// Build a provider from the configured API key and language. Fails only if the HTTP
    /// client cannot be constructed (rustls init).
    pub fn new(api_key: impl Into<String>, language: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("medi/0.1 (+https://github.com/michelve/medi)")
            .build()
            .map_err(|e| Error::Http(e.to_string()))?;
        Ok(Self {
            api_key: api_key.into(),
            language: language.into(),
            http,
            sem: Arc::new(Semaphore::new(MAX_CONCURRENCY)),
            image_base: OnceCell::new(),
            profile_base: OnceCell::new(),
        })
    }

    /// GET a TMDB endpoint with the api key + language, returning parsed JSON. Bounded by
    /// the semaphore so bursts never exceed [`MAX_CONCURRENCY`] in flight.
    async fn get(&self, path: &str, extra: &[(&str, &str)]) -> Result<Value> {
        let _permit = self.sem.acquire().await.expect("semaphore open");
        let url = format!("{API_BASE}{path}");
        let mut req = self
            .http
            .get(&url)
            .query(&[("api_key", self.api_key.as_str()), ("language", self.language.as_str())]);
        if !extra.is_empty() {
            req = req.query(extra);
        }
        let resp = req.send().await.map_err(|e| Error::Http(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Provider(format!("tmdb {path} → HTTP {status}")));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| Error::Parse(e.to_string()))
    }

    /// The `/configuration` image base URL, fetched once and cached for the process.
    async fn image_base(&self) -> String {
        self.image_base
            .get_or_init(|| async {
                match self.get("/configuration", &[]).await {
                    Ok(v) => parse_image_base(&v).unwrap_or_else(|| FALLBACK_IMAGE_BASE.to_string()),
                    Err(err) => {
                        tracing::warn!(error = %err, "tmdb /configuration failed; using fallback image base");
                        FALLBACK_IMAGE_BASE.to_string()
                    }
                }
            })
            .await
            .clone()
    }

    /// The `/configuration` profile-image base URL for headshots, fetched once and cached
    /// for the process (`docs/.tasks/91` Phase B). Reads the same `/configuration` response
    /// as [`Self::image_base`] but a different size list (`profile_sizes`).
    async fn profile_base(&self) -> String {
        self.profile_base
            .get_or_init(|| async {
                match self.get("/configuration", &[]).await {
                    Ok(v) => {
                        parse_profile_base(&v).unwrap_or_else(|| FALLBACK_PROFILE_BASE.to_string())
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "tmdb /configuration failed; using fallback profile base");
                        FALLBACK_PROFILE_BASE.to_string()
                    }
                }
            })
            .await
            .clone()
    }
}

#[async_trait]
impl MetadataProvider for TmdbProvider {
    fn name(&self) -> &'static str {
        "tmdb"
    }

    async fn search(&self, title: &str, year: Option<i64>, kind: MediaKind) -> Result<Vec<Match>> {
        let (path, year_key) = match kind {
            MediaKind::Movie => ("/search/movie", "year"),
            MediaKind::Series => ("/search/tv", "first_air_date_year"),
        };
        let mut extra: Vec<(&str, &str)> = vec![("query", title)];
        let year_str;
        if let Some(y) = year {
            year_str = y.to_string();
            extra.push((year_key, &year_str));
        }
        let json = self.get(path, &extra).await?;
        Ok(parse_search(&json, title, year, kind))
    }

    async fn details(&self, id: &ProviderId) -> Result<Details> {
        let (tmdb_id, kind) = match id {
            ProviderId::Tmdb { id, kind } => (*id, *kind),
            ProviderId::Imdb(_) => {
                return Err(Error::Provider(
                    "tmdb provider cannot fetch details for a bare IMDb id".into(),
                ))
            }
        };
        let path = match kind {
            MediaKind::Movie => format!("/movie/{tmdb_id}"),
            MediaKind::Series => format!("/tv/{tmdb_id}"),
        };
        let json = self
            .get(&path, &[("append_to_response", "credits,external_ids,videos")])
            .await?;
        let image_base = self.image_base().await;
        Ok(parse_details(&json, tmdb_id, &image_base))
    }

    async fn person_details(&self, person_tmdb_id: i64) -> Result<Option<PersonDetails>> {
        // `GET /person/{id}` — bounded by the same semaphore as every other call.
        let json = self.get(&format!("/person/{person_tmdb_id}"), &[]).await?;
        let profile_base = self.profile_base().await;
        Ok(Some(parse_person(&json, person_tmdb_id, &profile_base)))
    }
}

// ---------------------------------------------------------------------------
// Pure parsing (tested against recorded fixtures, no network)
// ---------------------------------------------------------------------------

/// Extract the poster/backdrop image base from a `/configuration` response
/// (`images.secure_base_url` + a width). Returns `None` if the shape is unexpected.
pub fn parse_image_base(v: &Value) -> Option<String> {
    let images = v.get("images")?;
    let base = images.get("secure_base_url")?.as_str()?;
    // Pick a mid-size width present in the config, else default to w780.
    let width = images
        .get("poster_sizes")
        .and_then(|s| s.as_array())
        .and_then(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .find(|w| *w == "w780")
                .or_else(|| arr.iter().filter_map(|x| x.as_str()).nth(arr.len().saturating_sub(2)))
        })
        .unwrap_or("w780");
    Some(format!("{}/{width}", base.trim_end_matches('/')))
}

/// Extract the profile-image base for headshots from a `/configuration` response
/// (`images.secure_base_url` + a `profile_sizes` width) — `docs/.tasks/91` Phase B. Prefers
/// `h632` (TMDB's tallest fixed profile width), else the second-largest available, else
/// `original`. Returns `None` if the shape is unexpected.
pub fn parse_profile_base(v: &Value) -> Option<String> {
    let images = v.get("images")?;
    let base = images.get("secure_base_url")?.as_str()?;
    let width = images
        .get("profile_sizes")
        .and_then(|s| s.as_array())
        .and_then(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .find(|w| *w == "h632")
                .or_else(|| arr.iter().filter_map(|x| x.as_str()).nth(arr.len().saturating_sub(2)))
        })
        .unwrap_or("h632");
    Some(format!("{}/{width}", base.trim_end_matches('/')))
}

/// Parse a `/person/{id}` response into [`PersonDetails`], resolving the `profile_path`
/// against `profile_base` (`docs/.tasks/91` Phase B). An empty `biography`/`profile_path`
/// becomes `None`. `tmdb_id` is passed in (the id we requested) so the shape stays a pure
/// function of the JSON + the base.
pub fn parse_person(v: &Value, tmdb_id: i64, profile_base: &str) -> PersonDetails {
    let name = v
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or_default()
        .to_string();
    let biography = v
        .get("biography")
        .and_then(|b| b.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let photo_url = v
        .get("profile_path")
        .and_then(|p| p.as_str())
        .filter(|s| !s.is_empty())
        .map(|p| format!("{profile_base}{p}"));
    PersonDetails {
        tmdb_id,
        name,
        biography,
        photo_url,
    }
}

/// Parse a `/search/{movie,tv}` response into scored [`Match`]es, ordered best-first.
///
/// The parsed `(title, year)` we searched for is passed back in so each candidate can be
/// scored with [`matcher::score`] rather than trusting TMDB's own ranking blindly.
pub fn parse_search(v: &Value, query_title: &str, query_year: Option<i64>, kind: MediaKind) -> Vec<Match> {
    let Some(results) = v.get("results").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    let mut matches: Vec<Match> = results
        .iter()
        .filter_map(|r| {
            let id = r.get("id")?.as_i64()?;
            // Movies use `title`/`release_date`; TV uses `name`/`first_air_date`.
            let (title_key, date_key) = match kind {
                MediaKind::Movie => ("title", "release_date"),
                MediaKind::Series => ("name", "first_air_date"),
            };
            let title = r.get(title_key).and_then(|t| t.as_str())?.to_string();
            let year = r
                .get(date_key)
                .and_then(|d| d.as_str())
                .and_then(year_from_date);
            let score = matcher::score(query_title, query_year, &title, year);
            Some(Match {
                provider_id: ProviderId::Tmdb { id, kind },
                title,
                year,
                score,
            })
        })
        .collect();
    matches.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    matches
}

/// Parse a `/{movie,tv}/{id}?append_to_response=credits,external_ids` response into
/// [`Details`], resolving poster/backdrop paths against `image_base`.
pub fn parse_details(v: &Value, tmdb_id: i64, image_base: &str) -> Details {
    let overview = v
        .get("overview")
        .and_then(|o| o.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let poster_url = v
        .get("poster_path")
        .and_then(|p| p.as_str())
        .map(|p| format!("{image_base}{p}"));
    let backdrop_url = v
        .get("backdrop_path")
        .and_then(|p| p.as_str())
        .map(|p| format!("{image_base}{p}"));

    // external_ids.imdb_id (TV) or top-level imdb_id (movie).
    let imdb_id = v
        .get("imdb_id")
        .and_then(|i| i.as_str())
        .or_else(|| {
            v.get("external_ids")
                .and_then(|e| e.get("imdb_id"))
                .and_then(|i| i.as_str())
        })
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let cast = parse_cast(v);
    let genres = parse_genres(v);
    let collection = parse_collection(v, image_base);
    let trailers = parse_trailers(v);

    Details {
        overview,
        cast,
        poster_url,
        backdrop_url,
        imdb_id,
        tmdb_id: Some(tmdb_id),
        genres,
        collection,
        trailers,
    }
}

/// Parse `belongs_to_collection: { id, name, poster_path }` from a `/movie/{id}` details
/// response into a [`Collection`], resolving the poster against `image_base`. `None` when the
/// field is absent/null (a standalone movie, or a TV title).
pub fn parse_collection(v: &Value, image_base: &str) -> Option<Collection> {
    let c = v.get("belongs_to_collection")?;
    if c.is_null() {
        return None;
    }
    let tmdb_id = c.get("id")?.as_i64()?;
    let name = c.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let poster_url = c
        .get("poster_path")
        .and_then(|p| p.as_str())
        .filter(|s| !s.is_empty())
        .map(|p| format!("{image_base}{p}"));
    Some(Collection {
        tmdb_id,
        name: name.to_string(),
        poster_url,
    })
}

/// Parse an appended `videos.results[]` block into YouTube [`TrailerIn`]s, best-first.
/// Keeps only `site == "YouTube"` entries with a non-empty `key` (the only site the client
/// embeds), preferring official Trailers, then Teasers, then everything else, and official
/// entries within each. Empty when the title has no YouTube videos.
pub fn parse_trailers(v: &Value) -> Vec<TrailerIn> {
    let Some(results) = v
        .get("videos")
        .and_then(|vids| vids.get("results"))
        .and_then(|r| r.as_array())
    else {
        return Vec::new();
    };

    // Collect YouTube videos with a rank so the best trailer surfaces first.
    let mut ranked: Vec<(i64, TrailerIn)> = results
        .iter()
        .filter(|entry| entry.get("site").and_then(|s| s.as_str()) == Some("YouTube"))
        .filter_map(|entry| {
            let key = entry.get("key").and_then(|k| k.as_str())?.trim();
            if key.is_empty() {
                return None;
            }
            let kind = entry
                .get("type")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string());
            let official = entry.get("official").and_then(|o| o.as_bool()).unwrap_or(false);
            // Lower rank sorts first: Trailer(0) < Teaser(2) < other(4); official beats not.
            let type_rank = match kind.as_deref() {
                Some("Trailer") => 0,
                Some("Teaser") => 2,
                _ => 4,
            };
            let rank = type_rank + if official { 0 } else { 1 };
            Some((
                rank,
                TrailerIn {
                    youtube_key: key.to_string(),
                    name: entry.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()),
                    kind,
                },
            ))
        })
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, t)| t).collect()
}

/// Extract the top-level `genres: [{id, name}]` array from a `/movie/{id}` or `/tv/{id}`
/// details response into [`Genre`]s. Zero extra HTTP — it reads the array already present
/// in the details payload the pipeline fetches. Entries missing an id or a non-empty name
/// are skipped.
pub fn parse_genres(v: &Value) -> Vec<Genre> {
    let Some(arr) = v.get("genres").and_then(|g| g.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|g| {
            let tmdb_id = g.get("id")?.as_i64()?;
            let name = g.get("name")?.as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            Some(Genre {
                tmdb_id,
                name: name.to_string(),
            })
        })
        .collect()
}

/// Extract billed cast (+ the director as a crew credit) from an appended `credits`
/// block, preserving TMDB's `order` as the billing order.
fn parse_cast(v: &Value) -> Vec<CreditIn> {
    let credits = match v.get("credits") {
        Some(c) => c,
        None => return Vec::new(),
    };
    let mut out = Vec::new();

    if let Some(cast) = credits.get("cast").and_then(|c| c.as_array()) {
        for member in cast {
            let Some(name) = member.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            let ord = member.get("order").and_then(|o| o.as_i64()).unwrap_or(out.len() as i64);
            let character = member
                .get("character")
                .and_then(|c| c.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let person_tmdb_id = member.get("id").and_then(|i| i.as_i64());
            out.push(CreditIn {
                name: name.to_string(),
                role: "actor".to_string(),
                character,
                ord,
                person_tmdb_id,
            });
        }
    }

    // Directors are billed after the cast; keep them out of the top-billing range by
    // continuing the ord sequence.
    if let Some(crew) = credits.get("crew").and_then(|c| c.as_array()) {
        let mut next_ord = out.len() as i64;
        for member in crew {
            let is_director = member.get("job").and_then(|j| j.as_str()) == Some("Director");
            if !is_director {
                continue;
            }
            let Some(name) = member.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            let person_tmdb_id = member.get("id").and_then(|i| i.as_i64());
            out.push(CreditIn {
                name: name.to_string(),
                role: "director".to_string(),
                character: None,
                ord: next_ord,
                person_tmdb_id,
            });
            next_ord += 1;
        }
    }

    out
}

/// The 4-digit year from a `YYYY-MM-DD` (or `YYYY`) date string.
fn year_from_date(date: &str) -> Option<i64> {
    date.get(0..4).and_then(|y| y.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_configuration_image_base() {
        let cfg = json!({
            "images": {
                "secure_base_url": "https://image.tmdb.org/t/p/",
                "poster_sizes": ["w92", "w154", "w342", "w500", "w780", "original"]
            }
        });
        assert_eq!(
            parse_image_base(&cfg).as_deref(),
            Some("https://image.tmdb.org/t/p/w780")
        );
    }

    #[test]
    fn parses_configuration_profile_base() {
        let cfg = json!({
            "images": {
                "secure_base_url": "https://image.tmdb.org/t/p/",
                "profile_sizes": ["w45", "w185", "h632", "original"]
            }
        });
        assert_eq!(
            parse_profile_base(&cfg).as_deref(),
            Some("https://image.tmdb.org/t/p/h632")
        );
    }

    #[test]
    fn parse_person_extracts_bio_and_photo() {
        let resp = json!({
            "id": 9273,
            "name": "Amy Adams",
            "biography": "  Amy Lou Adams is an American actress.  ",
            "profile_path": "/amy.jpg",
            "birthday": "1974-08-20"
        });
        let p = parse_person(&resp, 9273, "https://img/h632");
        assert_eq!(p.tmdb_id, 9273);
        assert_eq!(p.name, "Amy Adams");
        // Biography is trimmed.
        assert_eq!(p.biography.as_deref(), Some("Amy Lou Adams is an American actress."));
        assert_eq!(p.photo_url.as_deref(), Some("https://img/h632/amy.jpg"));
    }

    #[test]
    fn parse_collection_resolves_poster() {
        let resp = json!({
            "belongs_to_collection": {
                "id": 295,
                "name": "Pirates of the Caribbean Collection",
                "poster_path": "/poster.jpg"
            }
        });
        let c = parse_collection(&resp, "https://img/w780").unwrap();
        assert_eq!(c.tmdb_id, 295);
        assert_eq!(c.name, "Pirates of the Caribbean Collection");
        assert_eq!(c.poster_url.as_deref(), Some("https://img/w780/poster.jpg"));

        // A standalone movie (null / absent) → None.
        assert!(parse_collection(&json!({ "belongs_to_collection": null }), "x").is_none());
        assert!(parse_collection(&json!({}), "x").is_none());
    }

    #[test]
    fn parse_trailers_keeps_youtube_and_ranks_official_trailers_first() {
        let resp = json!({
            "videos": { "results": [
                { "site": "YouTube", "key": "clip1", "type": "Clip", "official": true, "name": "A clip" },
                { "site": "Vimeo",   "key": "vimeo1", "type": "Trailer", "official": true, "name": "ignored" },
                { "site": "YouTube", "key": "teaser1", "type": "Teaser", "official": true, "name": "Teaser" },
                { "site": "YouTube", "key": "trailer1", "type": "Trailer", "official": true, "name": "Official Trailer" },
                { "site": "YouTube", "key": "", "type": "Trailer", "official": true }
            ]}
        });
        let t = parse_trailers(&resp);
        // Vimeo dropped, empty-key dropped → 3 YouTube videos.
        assert_eq!(t.len(), 3);
        // Official Trailer ranks first, then Teaser, then the Clip.
        assert_eq!(t[0].youtube_key, "trailer1");
        assert_eq!(t[0].kind.as_deref(), Some("Trailer"));
        assert_eq!(t[1].youtube_key, "teaser1");
        assert_eq!(t[2].youtube_key, "clip1");
        // No videos block → empty.
        assert!(parse_trailers(&json!({})).is_empty());
    }

    #[test]
    fn parse_person_empty_fields_become_none() {
        let resp = json!({ "id": 1, "name": "No Photo", "biography": "", "profile_path": null });
        let p = parse_person(&resp, 1, "https://img/h632");
        assert_eq!(p.name, "No Photo");
        assert!(p.biography.is_none());
        assert!(p.photo_url.is_none());
    }

    #[test]
    fn search_scores_and_orders_candidates() {
        // Two candidates; the exact-year Arrival should rank first over a wrong-year one.
        let resp = json!({
            "results": [
                { "id": 1, "title": "Arrival", "release_date": "1996-01-01" },
                { "id": 329865, "title": "Arrival", "release_date": "2016-11-11" },
                { "id": 999, "title": "The Departed", "release_date": "2006-10-06" }
            ]
        });
        let matches = parse_search(&resp, "Arrival", Some(2016), MediaKind::Movie);
        assert_eq!(matches.len(), 3);
        // Best is the 2016 Arrival.
        assert_eq!(matches[0].title, "Arrival");
        assert_eq!(matches[0].year, Some(2016));
        assert_eq!(matches[0].provider_id, ProviderId::Tmdb { id: 329865, kind: MediaKind::Movie });
        assert!(matches[0].score >= matcher::MATCH_THRESHOLD);
        // The unrelated title scores lowest.
        assert_eq!(matches[2].title, "The Departed");
        assert!(matches[2].score < matches[1].score);
    }

    #[test]
    fn search_tv_uses_name_and_first_air_date() {
        let resp = json!({
            "results": [
                { "id": 95396, "name": "Severance", "first_air_date": "2022-02-18" }
            ]
        });
        let matches = parse_search(&resp, "Severance", Some(2022), MediaKind::Series);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].year, Some(2022));
        assert_eq!(
            matches[0].provider_id,
            ProviderId::Tmdb { id: 95396, kind: MediaKind::Series }
        );
    }

    #[test]
    fn details_extracts_overview_art_cast_and_ids() {
        let resp = json!({
            "id": 329865,
            "overview": "Linguist Louise Banks is recruited to communicate with aliens.",
            "poster_path": "/poster.jpg",
            "backdrop_path": "/backdrop.jpg",
            "imdb_id": "tt2543164",
            "genres": [
                { "id": 878, "name": "Science Fiction" },
                { "id": 18, "name": "Drama" }
            ],
            "credits": {
                "cast": [
                    { "id": 9273, "name": "Amy Adams", "character": "Louise Banks", "order": 0 },
                    { "id": 17647, "name": "Jeremy Renner", "character": "Ian Donnelly", "order": 1 }
                ],
                "crew": [
                    { "id": 137427, "name": "Denis Villeneuve", "job": "Director" },
                    { "name": "Someone Else", "job": "Producer" }
                ]
            }
        });
        let d = parse_details(&resp, 329865, "https://img/w780");
        assert!(d.overview.unwrap().starts_with("Linguist"));
        assert_eq!(d.poster_url.as_deref(), Some("https://img/w780/poster.jpg"));
        assert_eq!(d.backdrop_url.as_deref(), Some("https://img/w780/backdrop.jpg"));
        assert_eq!(d.imdb_id.as_deref(), Some("tt2543164"));
        assert_eq!(d.tmdb_id, Some(329865));
        assert_eq!(d.cast.len(), 3); // 2 actors + 1 director; the producer is dropped.
        assert_eq!(d.cast[0].name, "Amy Adams");
        assert_eq!(d.cast[0].role, "actor");
        assert_eq!(d.cast[0].character.as_deref(), Some("Louise Banks"));
        assert_eq!(d.cast[0].ord, 0);
        // Cast person ids are captured for person enrichment (`docs/.tasks/91` Phase B).
        assert_eq!(d.cast[0].person_tmdb_id, Some(9273));
        let director = d.cast.iter().find(|c| c.role == "director").unwrap();
        assert_eq!(director.name, "Denis Villeneuve");
        assert!(director.ord >= 2, "director billed after the cast");
        assert_eq!(director.person_tmdb_id, Some(137427));

        // Genres come free from the same details response (`docs/.tasks/91` Phase A).
        assert_eq!(d.genres.len(), 2);
        assert_eq!(d.genres[0].tmdb_id, 878);
        assert_eq!(d.genres[0].name, "Science Fiction");
        assert_eq!(d.genres[1].tmdb_id, 18);
    }

    #[test]
    fn genres_absent_or_malformed_yield_empty() {
        // No `genres` key → empty (a title with none, or an old response shape).
        assert!(parse_genres(&json!({})).is_empty());
        // Entries missing an id or a non-empty name are skipped.
        let resp = json!({
            "genres": [
                { "id": 28, "name": "Action" },
                { "name": "No Id" },
                { "id": 99, "name": "" }
            ]
        });
        let g = parse_genres(&resp);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].tmdb_id, 28);
        assert_eq!(g[0].name, "Action");
    }

    #[test]
    fn empty_search_results_is_empty() {
        assert!(parse_search(&json!({ "results": [] }), "X", None, MediaKind::Movie).is_empty());
        assert!(parse_search(&json!({}), "X", None, MediaKind::Movie).is_empty());
    }
}
