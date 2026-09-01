//! Rust row models + serde DTOs mirroring the `01-db-schema.md` tables.
//!
//! Each struct maps 1:1 to a table and knows how to build itself from a
//! [`rusqlite::Row`] via `from_row`. Columns are read positionally against the
//! explicit `SELECT` lists in [`crate::queries`]; keep the two in lockstep.
//!
//! The `media_files` HDR/DV columns are stored as loose strings/ints for
//! ffprobe-fidelity; [`MediaFile::profile`] reconstructs the typed
//! [`MediaProfile`] from `medi-core` for the transcode decision.

use medi_core::{DvProfile, HdrType, MediaProfile, VideoCodec};
use rusqlite::Row;
use serde::{Deserialize, Serialize};

/// A row of `movies`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Movie {
    pub id: i64,
    pub title: String,
    pub sort_title: String,
    pub year: Option<i64>,
    pub overview: Option<String>,
    pub added_at: i64,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
}

impl Movie {
    /// Column order: id, title, sort_title, year, overview, added_at,
    /// poster_path, backdrop_path.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            title: row.get(1)?,
            sort_title: row.get(2)?,
            year: row.get(3)?,
            overview: row.get(4)?,
            added_at: row.get(5)?,
            poster_path: row.get(6)?,
            backdrop_path: row.get(7)?,
        })
    }
}

/// A row of `series`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Series {
    pub id: i64,
    pub title: String,
    pub sort_title: String,
    pub year: Option<i64>,
    pub overview: Option<String>,
    pub added_at: i64,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
}

impl Series {
    /// Column order: id, title, sort_title, year, overview, added_at,
    /// poster_path, backdrop_path.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            title: row.get(1)?,
            sort_title: row.get(2)?,
            year: row.get(3)?,
            overview: row.get(4)?,
            added_at: row.get(5)?,
            poster_path: row.get(6)?,
            backdrop_path: row.get(7)?,
        })
    }
}

/// A row of `seasons`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Season {
    pub id: i64,
    pub series_id: i64,
    pub season_number: i64,
}

impl Season {
    /// Column order: id, series_id, season_number.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            series_id: row.get(1)?,
            season_number: row.get(2)?,
        })
    }
}

/// A row of `episodes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Episode {
    pub id: i64,
    pub season_id: i64,
    pub episode_number: i64,
    pub title: Option<String>,
    pub overview: Option<String>,
}

impl Episode {
    /// Column order: id, season_id, episode_number, title, overview.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            season_id: row.get(1)?,
            episode_number: row.get(2)?,
            title: row.get(3)?,
            overview: row.get(4)?,
        })
    }
}

/// A row of `media_files`. Belongs to exactly one movie OR one episode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaFile {
    pub id: i64,
    pub movie_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub path: String,
    pub container: Option<String>,
    pub size_bytes: Option<i64>,
    pub duration_ms: Option<i64>,
    // video stream
    pub video_codec: Option<String>,
    pub video_profile: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub bit_depth: Option<i64>,
    pub bitrate: Option<i64>,
    // HDR / color
    pub transfer_characteristics: Option<String>,
    pub color_space: Option<String>,
    pub hdr_type: Option<String>,
    // Dolby Vision
    pub dv_profile: Option<i64>,
    pub dv_bl_compatible_id: Option<i64>,
    pub dv_level: Option<i64>,
    pub hw_decode_unsupported: bool,
}

impl MediaFile {
    /// Column order matches [`crate::queries::MEDIA_FILE_COLUMNS`].
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            movie_id: row.get(1)?,
            episode_id: row.get(2)?,
            path: row.get(3)?,
            container: row.get(4)?,
            size_bytes: row.get(5)?,
            duration_ms: row.get(6)?,
            video_codec: row.get(7)?,
            video_profile: row.get(8)?,
            width: row.get(9)?,
            height: row.get(10)?,
            bit_depth: row.get(11)?,
            bitrate: row.get(12)?,
            transfer_characteristics: row.get(13)?,
            color_space: row.get(14)?,
            hdr_type: row.get(15)?,
            dv_profile: row.get(16)?,
            dv_bl_compatible_id: row.get(17)?,
            dv_level: row.get(18)?,
            // stored as 0/1
            hw_decode_unsupported: row.get::<_, i64>(19)? != 0,
        })
    }

    /// Reconstruct the typed [`MediaProfile`] the transcode crate consumes.
    ///
    /// Returns `None` if the row has not been probed yet (no `width`/`height`),
    /// since a profile without dimensions is meaningless to the decision logic.
    pub fn profile(&self) -> Option<MediaProfile> {
        let width = self.width? as u32;
        let height = self.height? as u32;

        let codec = match self.video_codec.as_deref() {
            Some("h264") => VideoCodec::H264,
            Some("hevc") => VideoCodec::Hevc,
            Some("av1") => VideoCodec::Av1,
            _ => VideoCodec::Other,
        };

        let hdr = match self.hdr_type.as_deref() {
            Some("hdr10") => HdrType::Hdr10,
            Some("hdr10plus") => HdrType::Hdr10Plus,
            Some("hlg") => HdrType::Hlg,
            Some("dolbyvision") => HdrType::DolbyVision,
            _ => HdrType::None,
        };

        let dv = match self.dv_profile {
            Some(5) => Some(DvProfile::P5),
            Some(7) => Some(DvProfile::P7),
            Some(8) => Some(DvProfile::P8 {
                bl_compatible_id: self.dv_bl_compatible_id.unwrap_or(0) as u8,
            }),
            _ => None,
        };

        Some(MediaProfile {
            codec,
            width,
            height,
            bit_depth: self.bit_depth.unwrap_or(8) as u8,
            hdr,
            dv,
            hw_decode_unsupported: self.hw_decode_unsupported,
        })
    }
}

/// A row of `people`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    pub id: i64,
    pub name: String,
}

/// A row of `libraries` (`docs/.tasks/60` Phase B).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    pub id: i64,
    pub name: String,
    /// `"movie"` | `"series"`.
    pub kind: String,
    pub created_at: i64,
}

impl Library {
    /// Column order: id, name, kind, created_at.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            kind: row.get(2)?,
            created_at: row.get(3)?,
        })
    }
}

/// A library together with its folder paths — the shape `GET /api/libraries` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryWithFolders {
    #[serde(flatten)]
    pub library: Library,
    pub folders: Vec<String>,
}

/// A joined `credits` + `people` row (billing entry for a title).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credit {
    pub id: i64,
    pub person_id: i64,
    pub person_name: String,
    pub role: Option<String>,
    pub character: Option<String>,
    pub ord: Option<i64>,
}

impl Credit {
    /// Column order: credits.id, credits.person_id, people.name,
    /// credits.role, credits.character, credits.ord.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            person_id: row.get(1)?,
            person_name: row.get(2)?,
            role: row.get(3)?,
            character: row.get(4)?,
            ord: row.get(5)?,
        })
    }
}

/// A row of `preview_clips`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewClip {
    pub media_file_id: i64,
    pub path: String,
    pub generated_at: i64,
}

impl PreviewClip {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            media_file_id: row.get(0)?,
            path: row.get(1)?,
            generated_at: row.get(2)?,
        })
    }
}

/// A row of `trickplay_assets`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrickplayAsset {
    pub media_file_id: i64,
    pub kind: String,
    pub path: String,
    pub interval_ms: i64,
    pub tile_w: Option<i64>,
    pub tile_h: Option<i64>,
    pub cols: Option<i64>,
    pub rows: Option<i64>,
    pub generated_at: i64,
}

impl TrickplayAsset {
    /// Column order: media_file_id, kind, path, interval_ms, tile_w, tile_h,
    /// cols, rows, generated_at.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            media_file_id: row.get(0)?,
            kind: row.get(1)?,
            path: row.get(2)?,
            interval_ms: row.get(3)?,
            tile_w: row.get(4)?,
            tile_h: row.get(5)?,
            cols: row.get(6)?,
            rows: row.get(7)?,
            generated_at: row.get(8)?,
        })
    }
}

/// A row of `scan_state` (ingest/assets bookkeeping).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanState {
    pub path: String,
    pub mtime: i64,
    pub size_bytes: i64,
    pub probed_at: Option<i64>,
    pub preview_done_at: Option<i64>,
    pub trickplay_done_at: Option<i64>,
}

impl ScanState {
    /// Column order: path, mtime, size_bytes, probed_at, preview_done_at,
    /// trickplay_done_at.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            path: row.get(0)?,
            mtime: row.get(1)?,
            size_bytes: row.get(2)?,
            probed_at: row.get(3)?,
            preview_done_at: row.get(4)?,
            trickplay_done_at: row.get(5)?,
        })
    }
}

// ---------------------------------------------------------------------------
// DTOs — the composed shapes the `api` crate serializes to the TV client.
// (Response envelopes like /api/library live in the api crate; these are the
// reusable detail aggregates the queries return.)
// ---------------------------------------------------------------------------

/// Full movie detail: the movie plus its files and billed credits.
/// Backs `GET /api/movies/:id` (see `02-api-contract.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovieDetail {
    #[serde(flatten)]
    pub movie: Movie,
    pub media_files: Vec<MediaFile>,
    pub credits: Vec<Credit>,
}

/// A season together with its ordered episodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeasonWithEpisodes {
    #[serde(flatten)]
    pub season: Season,
    pub episodes: Vec<Episode>,
}

/// Full series detail: the series plus its seasons/episodes and billed credits.
/// Backs `GET /api/series/:id` (see `02-api-contract.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesDetail {
    #[serde(flatten)]
    pub series: Series,
    pub seasons: Vec<SeasonWithEpisodes>,
    pub credits: Vec<Credit>,
}

/// Whether a [`LibraryCard`] describes a movie or a series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryKind {
    Movie,
    Series,
}

/// One card in the unified `/api/library` grid: a movie or a series, plus just the
/// fields the client needs to render a poster tile. The `api` crate maps this to the
/// public card shape (`kind`, `id`, `title`, `year`, `poster`, `hdr`).
///
/// `hdr` is the highest HDR tier found across the title's media files (a series shows
/// the strongest format any of its episodes carry), or `None` when unprobed / SDR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryCard {
    pub kind: LibraryKind,
    pub id: i64,
    pub title: String,
    pub sort_title: String,
    pub year: Option<i64>,
    pub added_at: i64,
    pub poster_path: Option<String>,
    pub hdr: Option<String>,
}

impl LibraryCard {
    /// Column order: kind_tag (0=movie,1=series), id, title, sort_title, year,
    /// added_at, poster_path, hdr.
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let kind = match row.get::<_, i64>(0)? {
            1 => LibraryKind::Series,
            _ => LibraryKind::Movie,
        };
        Ok(Self {
            kind,
            id: row.get(1)?,
            title: row.get(2)?,
            sort_title: row.get(3)?,
            year: row.get(4)?,
            added_at: row.get(5)?,
            poster_path: row.get(6)?,
            hdr: row.get(7)?,
        })
    }
}
