//! The [`MetadataProvider`] trait plus the provider-agnostic types the enrichment
//! orchestration ([`crate::enrich`]) speaks in. Concrete providers ([`crate::tmdb`],
//! [`crate::omdb`]) translate their own JSON into these shapes so the rest of the crate
//! never knows which service answered (`docs/.tasks/60` §Sub-tasks 1).

use async_trait::async_trait;

use crate::Result;

/// Which kind of title we are searching for. A library's `kind` (Phase B) or the
/// filename classification (Phase A) picks this, and it selects the provider endpoint
/// (`/search/movie` vs `/search/tv` on TMDB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Movie,
    Series,
}

impl MediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKind::Movie => "movie",
            MediaKind::Series => "series",
        }
    }
}

/// An opaque provider-specific identity for a title. TMDB uses a numeric id; OMDb uses
/// an IMDb id string. Kept as a typed wrapper so the two never get mixed up and so a
/// pinned match (`POST /api/movies/:id/match`) round-trips through the API unambiguously.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderId {
    /// TMDB numeric id for the given kind.
    Tmdb { id: i64, kind: MediaKind },
    /// An IMDb id (`tt\d+`), used directly by OMDb and derivable from TMDB.
    Imdb(String),
}

impl ProviderId {
    /// The wire form used in `GET /api/movies/:id/matches` responses and accepted back
    /// by `POST /api/movies/:id/match`. `tmdb:movie:603` / `imdb:tt0133093`.
    pub fn to_token(&self) -> String {
        match self {
            ProviderId::Tmdb { id, kind } => format!("tmdb:{}:{}", kind.as_str(), id),
            ProviderId::Imdb(s) => format!("imdb:{s}"),
        }
    }

    /// Parse a token produced by [`Self::to_token`]. Returns `None` on a malformed token.
    pub fn from_token(token: &str) -> Option<Self> {
        let mut parts = token.splitn(3, ':');
        match parts.next()? {
            "tmdb" => {
                let kind = match parts.next()? {
                    "movie" => MediaKind::Movie,
                    "series" => MediaKind::Series,
                    _ => return None,
                };
                let id = parts.next()?.parse().ok()?;
                Some(ProviderId::Tmdb { id, kind })
            }
            "imdb" => Some(ProviderId::Imdb(parts.next()?.to_string())),
            _ => None,
        }
    }
}

/// A candidate returned by [`MetadataProvider::search`]. `score` is the provider- or
/// matcher-computed confidence in `[0.0, 1.0]`; enrichment picks the best-scoring
/// candidate above a threshold (see [`crate::enrich`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub provider_id: ProviderId,
    pub title: String,
    pub year: Option<i64>,
    pub score: f64,
}

/// A single cast/crew member from [`Details::cast`], flowing into `people` + `credits`.
/// `ord` is the provider's billing order (0 = top-billed), persisted into `credits.ord`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreditIn {
    pub name: String,
    /// `"actor"`, `"director"`, … Persisted into `credits.role`.
    pub role: String,
    /// The character played, for actors. `None` for crew.
    pub character: Option<String>,
    pub ord: i64,
}

/// The full descriptive metadata for one matched title, ready to write to the catalog.
/// URLs are absolute and provider-resolved (TMDB's image base is applied by the
/// provider), so [`crate::enrich`] can download them without knowing the provider.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Details {
    pub overview: Option<String>,
    pub cast: Vec<CreditIn>,
    /// Absolute URL of the poster image, or `None` if the title has none.
    pub poster_url: Option<String>,
    /// Absolute URL of the backdrop image, or `None`.
    pub backdrop_url: Option<String>,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<i64>,
}

/// A pluggable source of descriptive metadata. Implemented by [`crate::tmdb::TmdbProvider`]
/// (default) and [`crate::omdb::OmdbProvider`]. The API surface is deliberately tiny —
/// search for candidates, then fetch the details of a chosen one — so a third provider
/// is a small addition.
#[async_trait]
pub trait MetadataProvider: Send + Sync {
    /// A stable short name for logging (`"tmdb"` / `"omdb"`).
    fn name(&self) -> &'static str;

    /// Search the provider for candidates matching `title` (+ optional `year`).
    async fn search(
        &self,
        title: &str,
        year: Option<i64>,
        kind: MediaKind,
    ) -> Result<Vec<Match>>;

    /// Fetch the full details of a specific candidate.
    async fn details(&self, id: &ProviderId) -> Result<Details>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_id_token_round_trips() {
        let a = ProviderId::Tmdb { id: 603, kind: MediaKind::Movie };
        assert_eq!(a.to_token(), "tmdb:movie:603");
        assert_eq!(ProviderId::from_token("tmdb:movie:603"), Some(a));

        let b = ProviderId::Imdb("tt0133093".to_string());
        assert_eq!(b.to_token(), "imdb:tt0133093");
        assert_eq!(ProviderId::from_token("imdb:tt0133093"), Some(b));

        let s = ProviderId::Tmdb { id: 95, kind: MediaKind::Series };
        assert_eq!(ProviderId::from_token(&s.to_token()), Some(s));

        assert_eq!(ProviderId::from_token("bogus"), None);
        assert_eq!(ProviderId::from_token("tmdb:film:1"), None);
    }
}
