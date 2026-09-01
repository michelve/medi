//! Public response DTOs — the exact JSON shapes in
//! `docs/.tasks/02-api-contract.md` §Representative response shapes.
//!
//! Movie/series *detail* responses reuse the aggregates from `medi_db::models`
//! (`MovieDetail` / `SeriesDetail`) directly. This module holds the shapes that
//! differ from a raw row: the unified library card (which exposes a poster *URL*,
//! not a stored path) and the stream decision envelope.

use serde::Serialize;

use medi_db::models::{LibraryCard, LibraryKind};

/// One page of the unified catalog. `next_cursor` is `null` when the list is
/// exhausted (`docs/.tasks/02-api-contract.md`).
#[derive(Debug, Serialize)]
pub struct LibraryPage {
    pub items: Vec<LibraryItem>,
    pub next_cursor: Option<String>,
}

/// A single poster tile in `/api/library`.
#[derive(Debug, Serialize)]
pub struct LibraryItem {
    /// `"movie"` or `"series"`.
    pub kind: &'static str,
    pub id: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i64>,
    /// A ready-to-fetch `/api/images/...` URL, or `null` when the title has no art.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    /// Highest HDR tier across the title's files (`"dolbyvision"`, `"hdr10"`, …),
    /// omitted for SDR / unprobed titles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hdr: Option<String>,
}

impl LibraryItem {
    /// Build the public tile from a DB [`LibraryCard`], turning the stored
    /// `poster_path` into a client-facing `/api/images/<path>` URL.
    pub fn from_card(card: LibraryCard) -> Self {
        let kind = match card.kind {
            LibraryKind::Movie => "movie",
            LibraryKind::Series => "series",
        };
        Self {
            kind,
            id: card.id,
            title: card.title,
            year: card.year,
            poster: card.poster_path.map(image_url),
            hdr: card.hdr,
        }
    }
}

/// Turn a stored artwork path (relative to the images root, per `AppConfig`) into
/// the public URL the client fetches. Leading slashes are trimmed so the result is
/// always `/api/images/<clean path>`.
pub fn image_url(stored_path: String) -> String {
    format!("/api/images/{}", stored_path.trim_start_matches('/'))
}

/// The playback decision returned by `GET /api/stream/:file_id`.
///
/// `mode` is `"direct"` (client fetches `/api/direct/:file_id` with `Range`) or
/// `"hls"` (client opens the returned `url`, an `index.m3u8`). `reason` is a stable
/// slug for logs/debugging (e.g. `"dv_p5_sdr_display"`), produced by
/// `medi_transcode::Decision::reason`.
#[derive(Debug, Serialize)]
pub struct StreamDecision {
    pub file_id: i64,
    /// `"direct"` or `"hls"`.
    pub mode: &'static str,
    pub reason: &'static str,
    pub url: String,
}
