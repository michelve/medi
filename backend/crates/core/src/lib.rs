//! `medi-core` — shared types for the medi media server.
//!
//! These types flow across crate boundaries: `db` reads/writes them, `ingest`
//! produces them from `ffprobe`, `transcode` consumes them to pick a pipeline,
//! and `api` serializes them to the TV client. Per `docs/.tasks/00-architecture.md`,
//! they are defined ONCE here and never duplicated per crate.

pub mod config;
pub mod error;
pub mod profile;

pub use config::AppConfig;
pub use error::{Error, Result};
pub use profile::{
    AudioCodec, ClientCapabilities, DvProfile, HdrType, ImmersiveAudio, MediaProfile, Platform,
    QualityProfile, VideoCodec,
};
