//! The OMDb provider — a second implementation to validate the [`MetadataProvider`]
//! abstraction (`docs/.tasks/60` §Sub-tasks 3). TMDB stays the default.
//!
//! OMDb (`www.omdbapi.com`) is flatter than TMDB: one endpoint queried by title+year
//! (`?t=`, `?s=` for a search list) or by IMDb id (`?i=`). It returns a single object
//! for a `by-id`/`by-title` lookup and a `Search` array for a keyword search, and it
//! already embeds the poster URL and (comma-joined) actor list, so there is no separate
//! `/configuration` or `credits` round-trip.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::matcher;
use crate::provider::{CreditIn, Details, Match, MediaKind, MetadataProvider, ProviderId};
use crate::{Error, Result};

const API_BASE: &str = "https://www.omdbapi.com/";
const MAX_CONCURRENCY: usize = 8;

/// The OMDb provider.
pub struct OmdbProvider {
    api_key: String,
    http: reqwest::Client,
    sem: Arc<Semaphore>,
}

impl OmdbProvider {
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("medi/0.1 (+https://github.com/mvelis/medi)")
            .build()
            .map_err(|e| Error::Http(e.to_string()))?;
        Ok(Self {
            api_key: api_key.into(),
            http,
            sem: Arc::new(Semaphore::new(MAX_CONCURRENCY)),
        })
    }

    async fn get(&self, params: &[(&str, &str)]) -> Result<Value> {
        let _permit = self.sem.acquire().await.expect("semaphore open");
        let resp = self
            .http
            .get(API_BASE)
            .query(&[("apikey", self.api_key.as_str())])
            .query(params)
            .send()
            .await
            .map_err(|e| Error::Http(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Provider(format!("omdb → HTTP {status}")));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| Error::Parse(e.to_string()))
    }
}

#[async_trait]
impl MetadataProvider for OmdbProvider {
    fn name(&self) -> &'static str {
        "omdb"
    }

    async fn search(&self, title: &str, year: Option<i64>, kind: MediaKind) -> Result<Vec<Match>> {
        let type_param = match kind {
            MediaKind::Movie => "movie",
            MediaKind::Series => "series",
        };
        let mut params: Vec<(&str, &str)> = vec![("s", title), ("type", type_param)];
        let year_str;
        if let Some(y) = year {
            year_str = y.to_string();
            params.push(("y", &year_str));
        }
        let json = self.get(&params).await?;
        Ok(parse_search(&json, title, year))
    }

    async fn details(&self, id: &ProviderId) -> Result<Details> {
        let imdb = match id {
            ProviderId::Imdb(s) => s.clone(),
            ProviderId::Tmdb { .. } => {
                return Err(Error::Provider(
                    "omdb provider needs an IMDb id, not a TMDB id".into(),
                ))
            }
        };
        let json = self.get(&[("i", &imdb), ("plot", "full")]).await?;
        parse_details(&json)
    }
}

// ---------------------------------------------------------------------------
// Pure parsing (fixture-tested)
// ---------------------------------------------------------------------------

/// Parse an OMDb `?s=` search response (`{ "Search": [ { Title, Year, imdbID } ] }`)
/// into scored [`Match`]es, ordered best-first.
pub fn parse_search(v: &Value, query_title: &str, query_year: Option<i64>) -> Vec<Match> {
    let Some(results) = v.get("Search").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    let mut matches: Vec<Match> = results
        .iter()
        .filter_map(|r| {
            let imdb = r.get("imdbID")?.as_str()?.to_string();
            let title = r.get("Title")?.as_str()?.to_string();
            let year = r.get("Year").and_then(|y| y.as_str()).and_then(parse_year);
            let score = matcher::score(query_title, query_year, &title, year);
            Some(Match {
                provider_id: ProviderId::Imdb(imdb),
                title,
                year,
                score,
            })
        })
        .collect();
    matches.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    matches
}

/// Parse an OMDb `?i=` details response into [`Details`]. Errors if OMDb reports
/// `Response: "False"` (unknown id).
pub fn parse_details(v: &Value) -> Result<Details> {
    if v.get("Response").and_then(|r| r.as_str()) == Some("False") {
        let msg = v.get("Error").and_then(|e| e.as_str()).unwrap_or("not found");
        return Err(Error::Provider(format!("omdb: {msg}")));
    }

    let overview = v
        .get("Plot")
        .and_then(|p| p.as_str())
        .filter(|s| !s.is_empty() && *s != "N/A")
        .map(|s| s.to_string());

    let poster_url = v
        .get("Poster")
        .and_then(|p| p.as_str())
        .filter(|s| !s.is_empty() && *s != "N/A")
        .map(|s| s.to_string());

    let imdb_id = v
        .get("imdbID")
        .and_then(|i| i.as_str())
        .filter(|s| !s.is_empty() && *s != "N/A")
        .map(|s| s.to_string());

    // OMDb has no cast order; split the comma-joined Actors and assign an ascending ord.
    let mut cast = Vec::new();
    if let Some(actors) = v.get("Actors").and_then(|a| a.as_str()) {
        if actors != "N/A" {
            for (i, name) in actors.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).enumerate() {
                cast.push(CreditIn {
                    name: name.to_string(),
                    role: "actor".to_string(),
                    character: None,
                    ord: i as i64,
                });
            }
        }
    }
    // OMDb's Director is a comma list too.
    if let Some(directors) = v.get("Director").and_then(|d| d.as_str()) {
        if directors != "N/A" {
            let base = cast.len() as i64;
            for (i, name) in directors
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .enumerate()
            {
                cast.push(CreditIn {
                    name: name.to_string(),
                    role: "director".to_string(),
                    character: None,
                    ord: base + i as i64,
                });
            }
        }
    }

    Ok(Details {
        overview,
        cast,
        poster_url,
        // OMDb exposes no backdrop art.
        backdrop_url: None,
        imdb_id,
        tmdb_id: None,
    })
}

/// Parse OMDb's `Year` field, which may be `"2016"` or a series range `"2022–"`.
fn parse_year(raw: &str) -> Option<i64> {
    raw.get(0..4).and_then(|y| y.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn search_scores_and_orders() {
        let resp = json!({
            "Search": [
                { "Title": "Arrival", "Year": "2016", "imdbID": "tt2543164", "Type": "movie" },
                { "Title": "The Arrival", "Year": "1996", "imdbID": "tt0115571", "Type": "movie" }
            ],
            "Response": "True"
        });
        let matches = parse_search(&resp, "Arrival", Some(2016));
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].provider_id, ProviderId::Imdb("tt2543164".into()));
        assert_eq!(matches[0].year, Some(2016));
        assert!(matches[0].score > matches[1].score);
    }

    #[test]
    fn details_parses_plot_poster_actors() {
        let resp = json!({
            "Title": "Arrival",
            "Plot": "A linguist works with the military to communicate with aliens.",
            "Poster": "https://example.com/arrival.jpg",
            "Actors": "Amy Adams, Jeremy Renner, Forest Whitaker",
            "Director": "Denis Villeneuve",
            "imdbID": "tt2543164",
            "Response": "True"
        });
        let d = parse_details(&resp).unwrap();
        assert!(d.overview.unwrap().starts_with("A linguist"));
        assert_eq!(d.poster_url.as_deref(), Some("https://example.com/arrival.jpg"));
        assert_eq!(d.imdb_id.as_deref(), Some("tt2543164"));
        assert!(d.backdrop_url.is_none());
        // 3 actors + 1 director.
        assert_eq!(d.cast.len(), 4);
        assert_eq!(d.cast[0].name, "Amy Adams");
        assert_eq!(d.cast[0].ord, 0);
        assert_eq!(d.cast[3].role, "director");
    }

    #[test]
    fn details_na_fields_become_none() {
        let resp = json!({
            "Title": "Obscure",
            "Plot": "N/A",
            "Poster": "N/A",
            "Actors": "N/A",
            "Director": "N/A",
            "imdbID": "tt9999999",
            "Response": "True"
        });
        let d = parse_details(&resp).unwrap();
        assert!(d.overview.is_none());
        assert!(d.poster_url.is_none());
        assert!(d.cast.is_empty());
    }

    #[test]
    fn details_response_false_is_error() {
        let resp = json!({ "Response": "False", "Error": "Incorrect IMDb ID." });
        assert!(parse_details(&resp).is_err());
    }
}
