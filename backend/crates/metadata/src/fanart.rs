//! fanart.tv title-logo art client (`docs/.tasks/93`).
//!
//! fanart.tv is an **art-only** source keyed by the TMDB id the pipeline has already
//! resolved — not a [`crate::provider::MetadataProvider`] (it has no search/details
//! semantics). It plugs into [`crate::enrich`] as one extra art fetch, exactly like the
//! collection-poster and person-headshot downloads: given a movie's `tmdb_id`, fetch the
//! best transparent-PNG wordmark logo URL, download it locally, and serve it like a poster.
//!
//! JSON parsing is split into a pure [`parse_movie_logo`] tested against inline fixtures
//! with no live network, matching the `tmdb.rs` `parse_*` pattern. Concurrency is bounded
//! by a [`tokio::sync::Semaphore`] so a backfill of a large library never bursts fanart.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::{Error, Result};

/// fanart.tv's stable v3 API base. `/movies/{id}` accepts a TMDB (or IMDb) id.
const API_BASE: &str = "https://webservice.fanart.tv/v3";
/// Max concurrent fanart requests, mirroring `TmdbProvider`. fanart's personal-key limit is
/// generous but finite; the enrichment worker also bounds its own fan-out.
const MAX_CONCURRENCY: usize = 8;

/// The fanart.tv art medi consumes for one movie, from a single `/v3/movies/{id}` response
/// (`docs/.tasks/93` logos, `docs/.tasks/95` wallpapers). Each field is the best absolute
/// `https://assets.fanart.tv/…` URL of its kind, or `None` when fanart has no such art. Both
/// `None` (no logo, no wallpaper) is represented by the `FanartLookup::art` being `None` — see
/// [`FanartArt::movie_art`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MovieArt {
    /// The transparent-PNG title wordmark (`hdmovielogo` → `movielogo`).
    pub logo_url: Option<String>,
    /// The background wallpaper (`moviebackground`) — a 1920×1080 key-art background shown on
    /// the detail hero in place of the TMDB backdrop when present (`docs/.tasks/95`).
    pub wallpaper_url: Option<String>,
}

/// A source of a movie's fanart art (logo + wallpaper) by TMDB id, in **one** request.
/// Implemented by [`FanartClient`]; tests inject a stub returning canned URLs (mirroring
/// `ImageFetcher`), so the enrichment logic runs with no live network.
#[async_trait]
pub trait FanartArt: Send + Sync {
    /// The best logo + wallpaper for a TMDB movie id, or `None` (incl. on a 404 — fanart has
    /// no art for this id at all). A present [`MovieArt`] may still carry `None` fields when
    /// fanart has some art types but not others. Never errors on "no art"; only on a real
    /// HTTP/parse failure so the caller can log-and-continue.
    async fn movie_art(&self, tmdb_id: i64) -> Result<Option<MovieArt>>;
}

/// The production fanart.tv art client.
pub struct FanartClient {
    api_key: String,
    /// The language subtag preferred when picking among multiple logos (e.g. `"en"` from a
    /// configured `metadata_language` of `"en-US"`).
    preferred_lang: String,
    http: reqwest::Client,
    sem: Arc<Semaphore>,
}

impl FanartClient {
    /// Build a client from the fanart.tv key + the configured metadata language. Fails only
    /// if the HTTP client cannot be constructed (rustls init). The language's subtag before
    /// the first `-` becomes the logo-language preference (`"en-US"` → `"en"`).
    pub fn new(api_key: impl Into<String>, metadata_language: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("medi/0.1 (+https://github.com/michelve/medi)")
            .build()
            .map_err(|e| Error::Http(e.to_string()))?;
        Ok(Self {
            api_key: api_key.into(),
            preferred_lang: lang_subtag(metadata_language),
            http,
            sem: Arc::new(Semaphore::new(MAX_CONCURRENCY)),
        })
    }
}

#[async_trait]
impl FanartArt for FanartClient {
    async fn movie_art(&self, tmdb_id: i64) -> Result<Option<MovieArt>> {
        let _permit = self.sem.acquire().await.expect("semaphore open");
        let url = format!("{API_BASE}/movies/{tmdb_id}");
        let resp = self
            .http
            .get(&url)
            .query(&[("api_key", self.api_key.as_str())])
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;
        // 404 ⇒ fanart has no art for this id — a normal "no art", not an error.
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(Error::Provider(format!(
                "fanart /movies/{tmdb_id} → HTTP {}",
                resp.status()
            )));
        }
        let json = resp
            .json::<Value>()
            .await
            .map_err(|e| Error::Parse(e.to_string()))?;
        // One response, both art types — no second request.
        Ok(Some(MovieArt {
            logo_url: parse_movie_logo(&json, &self.preferred_lang),
            wallpaper_url: parse_movie_wallpaper(&json, &self.preferred_lang),
        }))
    }
}

/// The language subtag before the first `-` (`"en-US"` → `"en"`, `"fr"` → `"fr"`),
/// lowercased. Used to match fanart's `lang` field.
fn lang_subtag(language: &str) -> String {
    language
        .split(['-', '_'])
        .next()
        .unwrap_or(language)
        .to_ascii_lowercase()
}

/// Pick the best logo URL from a fanart.tv `/movies/{id}` response — a pure function of the
/// JSON + the preferred language subtag, so tests need no network (`docs/.tasks/93`).
///
/// Selection rule:
/// 1. Prefer the `hdmovielogo` array (HD wordmark), else `movielogo` (standard-res).
/// 2. Within the chosen array, prefer an entry whose `lang` matches `preferred_lang`; else
///    prefer `lang == "en"`; else the first entry. Tie-break by highest `likes` (fanart
///    stores `likes` as a *string*; parse to i64, default 0 on empty/missing).
/// 3. Return the chosen absolute `url` (already `https://assets.fanart.tv/…`), or `None`
///    when neither array has a usable entry.
pub fn parse_movie_logo(v: &Value, preferred_lang: &str) -> Option<String> {
    let entries = v
        .get("hdmovielogo")
        .and_then(|a| a.as_array())
        .filter(|a| !a.is_empty())
        .or_else(|| {
            v.get("movielogo")
                .and_then(|a| a.as_array())
                .filter(|a| !a.is_empty())
        })?;
    best_by_lang_and_likes(entries, preferred_lang)
}

/// Pick the best background wallpaper URL from a fanart.tv `/movies/{id}` response
/// (`docs/.tasks/95`) — a pure function of the JSON + preferred language, like
/// [`parse_movie_logo`]. Reads the `moviebackground` array (fanart's 1920×1080 key-art
/// backgrounds; the site's "wallpaper" section). Wallpapers usually carry no `lang`, so the
/// language tiers collapse to a pure highest-`likes` tie-break, which is the intended
/// behavior. Returns the chosen absolute `url`, or `None` when there are no backgrounds.
pub fn parse_movie_wallpaper(v: &Value, preferred_lang: &str) -> Option<String> {
    let entries = v
        .get("moviebackground")
        .and_then(|a| a.as_array())
        .filter(|a| !a.is_empty())?;
    best_by_lang_and_likes(entries, preferred_lang)
}

/// Pick the best entry from a fanart art array by `(language, likes)`: a preferred-language
/// match beats an English match beats anything; within a tier, higher `likes` wins; array
/// order is the final tie-break. Returns the chosen absolute `url`, or `None` when no entry
/// has a usable `url`. Shared by [`parse_movie_logo`] and [`parse_movie_wallpaper`] so the
/// selection rule lives in one place. `likes` is stored by fanart as a *string* (empty/missing
/// → 0); entries with no `lang` sort into the lowest tier.
fn best_by_lang_and_likes(entries: &[Value], preferred_lang: &str) -> Option<String> {
    let pref = preferred_lang.to_ascii_lowercase();
    entries
        .iter()
        .filter_map(|e| {
            let url = e.get("url").and_then(|u| u.as_str())?.trim();
            if url.is_empty() {
                return None;
            }
            let lang = e
                .get("lang")
                .and_then(|l| l.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let likes = e
                .get("likes")
                .and_then(|l| l.as_str())
                .and_then(|s| s.trim().parse::<i64>().ok())
                .unwrap_or(0);
            let lang_rank = if !pref.is_empty() && lang == pref {
                2
            } else if lang == "en" {
                1
            } else {
                0
            };
            Some((lang_rank, likes, url.to_string()))
        })
        // Stable: on an equal (lang_rank, likes) key the earlier array element is kept.
        .fold(None::<(i64, i64, String)>, |acc, cand| match acc {
            Some(best) if (best.0, best.1) >= (cand.0, cand.1) => Some(best),
            _ => Some(cand),
        })
        .map(|(_, _, url)| url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A trimmed copy of a real `GET /v3/movies/597` (Titanic) response — a couple of entries
    /// per array, verified live (session 2026-09-02) against the dev fanart key.
    fn titanic_response() -> Value {
        json!({
            "name": "Titanic",
            "tmdb_id": "597",
            "imdb_id": "tt0120338",
            "hdmovielogo": [
                { "id": "3321", "url": "https://assets.fanart.tv/fanart/movies/597/hdmovielogo/titanic-en.png", "lang": "en", "likes": "11" },
                { "id": "9988", "url": "https://assets.fanart.tv/fanart/movies/597/hdmovielogo/titanic-fr.png", "lang": "fr", "likes": "3" }
            ],
            "movielogo": [
                { "id": "111", "url": "https://assets.fanart.tv/fanart/movies/597/movielogo/titanic-sd.png", "lang": "en", "likes": "2" }
            ],
            "moviebackground": [
                { "id": "5501", "url": "https://assets.fanart.tv/fanart/movies/597/moviebackground/titanic-a.jpg", "lang": "", "likes": "7" },
                { "id": "5502", "url": "https://assets.fanart.tv/fanart/movies/597/moviebackground/titanic-b.jpg", "lang": "", "likes": "22" }
            ]
        })
    }

    #[test]
    fn prefers_hd_over_standard_logo() {
        let url = parse_movie_logo(&titanic_response(), "en").unwrap();
        assert!(url.contains("hdmovielogo"), "HD array wins over movielogo: {url}");
        assert!(url.ends_with("titanic-en.png"));
    }

    #[test]
    fn falls_back_to_movielogo_when_no_hd() {
        let resp = json!({
            "movielogo": [
                { "id": "111", "url": "https://assets.fanart.tv/sd.png", "lang": "en", "likes": "2" }
            ]
        });
        assert_eq!(
            parse_movie_logo(&resp, "en").as_deref(),
            Some("https://assets.fanart.tv/sd.png")
        );
    }

    #[test]
    fn prefers_configured_language_then_english() {
        // Preferred language (fr) wins even against a higher-liked English entry.
        let resp = json!({
            "hdmovielogo": [
                { "id": "1", "url": "https://a/en.png", "lang": "en", "likes": "50" },
                { "id": "2", "url": "https://a/fr.png", "lang": "fr", "likes": "1" }
            ]
        });
        assert_eq!(parse_movie_logo(&resp, "fr").as_deref(), Some("https://a/fr.png"));
        // With no French preference, English wins over an untagged/other language.
        let resp2 = json!({
            "hdmovielogo": [
                { "id": "1", "url": "https://a/de.png", "lang": "de", "likes": "50" },
                { "id": "2", "url": "https://a/en.png", "lang": "en", "likes": "1" }
            ]
        });
        assert_eq!(parse_movie_logo(&resp2, "es").as_deref(), Some("https://a/en.png"));
    }

    #[test]
    fn ties_break_on_likes_within_a_language_tier() {
        let resp = json!({
            "hdmovielogo": [
                { "id": "1", "url": "https://a/low.png", "lang": "en", "likes": "4" },
                { "id": "2", "url": "https://a/high.png", "lang": "en", "likes": "42" }
            ]
        });
        assert_eq!(parse_movie_logo(&resp, "en").as_deref(), Some("https://a/high.png"));
    }

    #[test]
    fn missing_or_empty_likes_defaults_to_zero() {
        // Neither entry has a preferred/English lang, so it's a pure likes tie-break; the
        // entry with parseable likes beats the empty-likes one (treated as 0).
        let resp = json!({
            "hdmovielogo": [
                { "id": "1", "url": "https://a/nolikes.png", "lang": "de", "likes": "" },
                { "id": "2", "url": "https://a/haslikes.png", "lang": "de", "likes": "7" }
            ]
        });
        assert_eq!(parse_movie_logo(&resp, "en").as_deref(), Some("https://a/haslikes.png"));
    }

    #[test]
    fn none_when_no_logo_arrays() {
        assert!(parse_movie_logo(&json!({}), "en").is_none());
        // Empty arrays are treated as absent.
        assert!(parse_movie_logo(&json!({ "hdmovielogo": [], "movielogo": [] }), "en").is_none());
        // Other art types present but no logos → None.
        let resp = json!({ "movieposter": [{ "id": "1", "url": "https://a/p.png", "lang": "en", "likes": "1" }] });
        assert!(parse_movie_logo(&resp, "en").is_none());
    }

    #[test]
    fn lang_subtag_takes_head() {
        assert_eq!(lang_subtag("en-US"), "en");
        assert_eq!(lang_subtag("fr"), "fr");
        assert_eq!(lang_subtag("pt_BR"), "pt");
        assert_eq!(lang_subtag("DE-de"), "de");
    }

    // -- wallpapers (`docs/.tasks/95`) --------------------------------------

    #[test]
    fn wallpaper_picks_highest_likes() {
        // moviebackground entries have no lang, so it's a pure likes tie-break: b (22) > a (7).
        let url = parse_movie_wallpaper(&titanic_response(), "en").unwrap();
        assert!(url.ends_with("titanic-b.jpg"), "highest-liked wallpaper wins: {url}");
    }

    #[test]
    fn wallpaper_none_when_no_backgrounds() {
        // A response with logos but no moviebackground → no wallpaper.
        let resp = json!({
            "hdmovielogo": [{ "id": "1", "url": "https://a/logo.png", "lang": "en", "likes": "1" }]
        });
        assert!(parse_movie_wallpaper(&resp, "en").is_none());
        // Empty array treated as absent.
        assert!(parse_movie_wallpaper(&json!({ "moviebackground": [] }), "en").is_none());
        assert!(parse_movie_wallpaper(&json!({}), "en").is_none());
    }

    #[test]
    fn wallpaper_prefers_configured_language_when_tagged() {
        // Rare, but some backgrounds carry a lang; the preferred language still wins a tier.
        let resp = json!({
            "moviebackground": [
                { "id": "1", "url": "https://a/en.jpg", "lang": "en", "likes": "99" },
                { "id": "2", "url": "https://a/fr.jpg", "lang": "fr", "likes": "1" }
            ]
        });
        assert_eq!(parse_movie_wallpaper(&resp, "fr").as_deref(), Some("https://a/fr.jpg"));
    }
}
