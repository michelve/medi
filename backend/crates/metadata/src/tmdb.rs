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
use crate::provider::{CreditIn, Details, Match, MediaKind, MetadataProvider, ProviderId};
use crate::{Error, Result};

const API_BASE: &str = "https://api.themoviedb.org/3";
/// A safe fallback image base if `/configuration` cannot be reached. TMDB's canonical
/// CDN host; `w780` is a reasonable poster/backdrop width for a 10-foot UI.
const FALLBACK_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/w780";
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
}

impl TmdbProvider {
    /// Build a provider from the configured API key and language. Fails only if the HTTP
    /// client cannot be constructed (rustls init).
    pub fn new(api_key: impl Into<String>, language: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("medi/0.1 (+https://github.com/mvelis/medi)")
            .build()
            .map_err(|e| Error::Http(e.to_string()))?;
        Ok(Self {
            api_key: api_key.into(),
            language: language.into(),
            http,
            sem: Arc::new(Semaphore::new(MAX_CONCURRENCY)),
            image_base: OnceCell::new(),
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
            .get(&path, &[("append_to_response", "credits,external_ids")])
            .await?;
        let image_base = self.image_base().await;
        Ok(parse_details(&json, tmdb_id, &image_base))
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

    Details {
        overview,
        cast,
        poster_url,
        backdrop_url,
        imdb_id,
        tmdb_id: Some(tmdb_id),
    }
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
            out.push(CreditIn {
                name: name.to_string(),
                role: "actor".to_string(),
                character,
                ord,
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
            out.push(CreditIn {
                name: name.to_string(),
                role: "director".to_string(),
                character: None,
                ord: next_ord,
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
            "credits": {
                "cast": [
                    { "name": "Amy Adams", "character": "Louise Banks", "order": 0 },
                    { "name": "Jeremy Renner", "character": "Ian Donnelly", "order": 1 }
                ],
                "crew": [
                    { "name": "Denis Villeneuve", "job": "Director" },
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
        let director = d.cast.iter().find(|c| c.role == "director").unwrap();
        assert_eq!(director.name, "Denis Villeneuve");
        assert!(director.ord >= 2, "director billed after the cast");
    }

    #[test]
    fn empty_search_results_is_empty() {
        assert!(parse_search(&json!({ "results": [] }), "X", None, MediaKind::Movie).is_empty());
        assert!(parse_search(&json!({}), "X", None, MediaKind::Movie).is_empty());
    }
}
