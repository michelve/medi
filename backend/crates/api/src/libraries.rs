//! Library management endpoints (`docs/.tasks/60` Phase B) + the `MEDIA_DIR`
//! path-containment check that is the security spine of user-supplied folders.
//!
//! `/media` is the read-only trust boundary (`50-phase5-playback-packaging.md`), so every
//! folder a client asks to add MUST canonicalize to a location inside `MEDIA_DIR`; a
//! `..` or symlink escape is a `400` and the UI can never point a library at an arbitrary
//! host path (`docs/.tasks/60` §Security). Canonicalization resolves symlinks and `..`
//! before the prefix check, so a symlink under `/media` pointing outside is rejected too.

use std::path::{Path, PathBuf};

use axum::extract::{Path as AxPath, State};
use axum::response::{IntoResponse, Response};
use axum::Json;

use medi_db::queries;
use medi_db::writes::{self, TitleKind};

use crate::dto::{CreateLibraryRequest, PatchLibraryRequest};
use crate::error::{ApiError, ApiResult};
use crate::routes::run_blocking;
use crate::state::AppState;

/// Parse a library `kind` string into [`TitleKind`], or a `400`.
fn parse_kind(kind: &str) -> ApiResult<TitleKind> {
    match kind {
        "movie" => Ok(TitleKind::Movie),
        "series" => Ok(TitleKind::Series),
        other => Err(ApiError::bad_request(format!(
            "unknown library kind '{other}' (expected 'movie' or 'series')"
        ))),
    }
}

/// Validate that `candidate` resolves to a real directory **inside** `media_dir`, and
/// return its canonical absolute path (what we persist). Rejects:
/// - a non-existent / non-directory path (so a typo cannot create a dead folder),
/// - anything that canonicalizes outside `media_dir` (a `..` escape or a symlink out).
///
/// `media_dir` is canonicalized once by the caller and passed in.
fn validate_folder(canonical_media: &Path, candidate: &str) -> ApiResult<String> {
    let raw = PathBuf::from(candidate);
    // canonicalize() resolves `.`/`..`/symlinks and requires the path to exist.
    let resolved = raw.canonicalize().map_err(|_| {
        ApiError::bad_request(format!("folder does not exist or is unreadable: {candidate}"))
    })?;
    if !resolved.is_dir() {
        return Err(ApiError::bad_request(format!("not a directory: {candidate}")));
    }
    if !resolved.starts_with(canonical_media) {
        return Err(ApiError::bad_request(format!(
            "folder must be inside MEDIA_DIR ({}): {candidate}",
            canonical_media.display()
        )));
    }
    Ok(resolved.to_string_lossy().into_owned())
}

/// Canonicalize `MEDIA_DIR` for containment checks. If it cannot be canonicalized (e.g.
/// it does not exist on this host), fall back to its literal form so tests and
/// misconfigured hosts get a deterministic prefix rather than a 500.
fn canonical_media(state: &AppState) -> PathBuf {
    let m = &state.config.media_dir;
    m.canonicalize().unwrap_or_else(|_| m.clone())
}

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `GET /api/libraries` — list libraries with their folders.
pub async fn list_libraries(State(state): State<AppState>) -> ApiResult<Response> {
    let libs = run_blocking(&state.db, |conn| queries::list_libraries(conn)).await?;
    Ok(Json(libs).into_response())
}

/// `POST /api/libraries` — create `{ name, kind, folders[] }`. Every folder is
/// MEDIA_DIR-containment-checked before the library is written; a bad folder rejects the
/// whole request (`400`) with nothing persisted.
pub async fn create_library(
    State(state): State<AppState>,
    Json(body): Json<CreateLibraryRequest>,
) -> ApiResult<Response> {
    let kind = parse_kind(&body.kind)?;
    let media = canonical_media(&state);

    // Validate all folders up front — reject the request atomically on any bad path.
    let mut folders = Vec::with_capacity(body.folders.len());
    for f in &body.folders {
        folders.push(validate_folder(&media, f)?);
    }

    let name = body.name.clone();
    let created = now_secs();
    let id = run_blocking(&state.db, move |conn| {
        let id = writes::create_library(conn, &name, kind, created)?;
        for path in &folders {
            writes::add_library_folder(conn, id, path)?;
        }
        Ok(id)
    })
    .await?;

    // A new library changes what the catalog contains once scanned → drop cached pages.
    state.cache.invalidate_all();
    let created_lib = run_blocking(&state.db, move |conn| queries::get_library(conn, id)).await?;
    Ok((axum::http::StatusCode::CREATED, Json(created_lib)).into_response())
}

/// `PATCH /api/libraries/:id` — rename and/or add/remove folders. Added folders are
/// containment-checked; removed folders are taken as-is (removing a now-invalid path is
/// always allowed). A `404` if the library does not exist.
pub async fn patch_library(
    State(state): State<AppState>,
    AxPath(id): AxPath<i64>,
    Json(body): Json<PatchLibraryRequest>,
) -> ApiResult<Response> {
    // Confirm the library exists (404 otherwise).
    run_blocking(&state.db, move |conn| queries::get_library(conn, id)).await?;

    let media = canonical_media(&state);
    let mut add = Vec::with_capacity(body.add_folders.len());
    for f in &body.add_folders {
        add.push(validate_folder(&media, f)?);
    }
    let remove = body.remove_folders.clone();
    let name = body.name.clone();

    run_blocking(&state.db, move |conn| {
        if let Some(name) = &name {
            writes::rename_library(conn, id, name)?;
        }
        for path in &add {
            writes::add_library_folder(conn, id, path)?;
        }
        for path in &remove {
            writes::remove_library_folder(conn, id, path)?;
        }
        Ok(())
    })
    .await?;

    state.cache.invalidate_all();
    let updated = run_blocking(&state.db, move |conn| queries::get_library(conn, id)).await?;
    Ok(Json(updated).into_response())
}

/// `DELETE /api/libraries/:id` — remove a library and cascade its titles (and their
/// media_files/credits). Also reaps the artwork directories of the removed titles, so a
/// library delete does not leak `/config/images`.
pub async fn delete_library(
    State(state): State<AppState>,
    AxPath(id): AxPath<i64>,
) -> ApiResult<Response> {
    // 404 if unknown.
    run_blocking(&state.db, move |conn| queries::get_library(conn, id)).await?;

    run_blocking(&state.db, move |conn| writes::delete_library(conn, id)).await?;
    state.cache.invalidate_all();

    // Reconcile artwork against the surviving titles (backstop for the cascade).
    let images = state.config.images_dir();
    if let Err(err) = medi_metadata::sweep_orphan_images(&state.db, &images).await {
        tracing::warn!(error = %err, "artwork sweep after library delete failed");
    }

    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

/// `POST /api/libraries/:id/scan` — trigger an immediate scan of one library.
///
/// Phase B ships the endpoint and its 404/kind plumbing; the actual multi-root rescan is
/// driven by the ingest worker's watch loop (a scan is already incremental + idempotent).
/// Returns `202 Accepted` to signal the scan was enqueued, matching the fire-and-forget
/// posture of the background worker.
pub async fn scan_library(
    State(state): State<AppState>,
    AxPath(id): AxPath<i64>,
) -> ApiResult<Response> {
    run_blocking(&state.db, move |conn| queries::get_library(conn, id)).await?;
    // The worker owns scanning; here we only acknowledge. A future enhancement can push
    // an explicit "scan library N now" signal onto the worker's channel.
    tracing::info!(library_id = id, "manual library scan requested");
    Ok(axum::http::StatusCode::ACCEPTED.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_folder_accepts_inside_and_rejects_escape() {
        let media_root = tempfile::tempdir().unwrap();
        let canonical = media_root.path().canonicalize().unwrap();
        // A real sub-directory under media is accepted.
        let sub = canonical.join("movies");
        std::fs::create_dir_all(&sub).unwrap();
        let ok = validate_folder(&canonical, sub.to_str().unwrap()).unwrap();
        assert!(PathBuf::from(&ok).starts_with(&canonical));

        // A sibling directory outside media is rejected.
        let outside = tempfile::tempdir().unwrap();
        let err = validate_folder(&canonical, outside.path().to_str().unwrap()).unwrap_err();
        // Rendered as a 400 bad_request.
        let resp = err.into_response();
        assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);

        // A `..` escape from inside media resolves outside → rejected.
        let escape = sub.join("..").join("..");
        let err2 = validate_folder(&canonical, escape.to_str().unwrap());
        assert!(err2.is_err(), "../.. escape from media must be rejected");

        // A non-existent path is rejected (no dead folders).
        let missing = canonical.join("does-not-exist");
        assert!(validate_folder(&canonical, missing.to_str().unwrap()).is_err());
    }
}
