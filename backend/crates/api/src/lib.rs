//! `medi-api` — the axum HTTP + HLS server, as a library so integration tests can
//! drive the router in-process (`tower::ServiceExt::oneshot`) without binding a port.
//!
//! The `medi` binary (`src/main.rs`) is a thin wrapper: load config, open the DB,
//! build [`state::AppState`], and serve [`routes::router`]. Endpoint contract:
//! `docs/.tasks/02-api-contract.md`.

pub mod cache;
pub mod cursor;
pub mod dto;
pub mod error;
pub mod libraries;
pub mod routes;
pub mod state;
pub mod status;

pub use routes::{router, spawn_backfill};
pub use state::AppState;
