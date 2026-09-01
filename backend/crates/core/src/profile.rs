//! Media profile types shared schema → ingest → transcode → api.
//!
//! Field semantics mirror the `media_files` columns in `docs/.tasks/01-db-schema.md`.
//! The Dolby Vision mapping mirrors `docs/.tasks/10-phase1-foundation-data.md`.

use serde::{Deserialize, Serialize};

/// Video codec of the primary video stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
    Other,
}

/// HDR classification. `hdr_type` column in `media_files`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HdrType {
    None,
    Hdr10,
    Hdr10Plus,
    Hlg,
    DolbyVision,
}

/// Dolby Vision profile. Drives the transcode decision downstream (Phase 2).
///
/// - `P5` — proprietary IPTPQc2, no fallback layer → always transcode for SDR displays.
/// - `P7` — BL(HDR10)+EL, common in 4K Blu-ray rips.
/// - `P8` — carries a base-layer compatibility id (see [`DvProfile::P8`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "profile")]
pub enum DvProfile {
    P5,
    P7,
    /// `bl_compatible_id`: 1 = HDR10 fallback, 4 = SDR fallback (others reserved).
    P8 { bl_compatible_id: u8 },
}

impl DvProfile {
    /// The raw profile number (5, 7, 8) as stored in `media_files.dv_profile`.
    pub fn number(self) -> u8 {
        match self {
            DvProfile::P5 => 5,
            DvProfile::P7 => 7,
            DvProfile::P8 { .. } => 8,
        }
    }
}

/// The normalized description of a physical media file's video characteristics.
///
/// This is the in-memory shape shared across crates; the persisted form is the
/// `media_files` row (`docs/.tasks/01-db-schema.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaProfile {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub hdr: HdrType,
    /// Present only when `hdr == DolbyVision`.
    pub dv: Option<DvProfile>,
    /// True for formats hardware decoders cannot handle (e.g. H.264 High 10),
    /// forcing a software-decode transcode path. See Phase 2.
    pub hw_decode_unsupported: bool,
}
