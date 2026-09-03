//! The JSON error model from `docs/.tasks/02-api-contract.md` §Caching & error model.
//!
//! Every fallible handler returns [`ApiResult`]; an [`ApiError`] renders as
//! `{ "error": { "code": "...", "message": "..." } }` with the matching HTTP status.
//! `code` is a stable machine-readable slug the client can branch on; `message` is
//! human-facing and may change.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// Result alias for API handlers.
pub type ApiResult<T> = std::result::Result<T, ApiError>;

/// A handler error carrying an HTTP status, a stable `code`, and a `message`.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// `404` — a requested resource (movie, series, file, asset) does not exist.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    /// `400` — the request was malformed (bad cursor, bad `sort`, unparseable id).
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    /// `409` — a transcode session is busy / capacity is exhausted (Phase 2).
    pub fn busy(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "busy", message)
    }

    /// `503` — the database is briefly unavailable (e.g. a scan holds the write lock).
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message)
    }

    /// `415` — the resource exists but cannot be served in the requested form (an image
    /// subtitle cannot be converted to WebVTT; the client must request a burn-in instead —
    /// `docs/.tasks/90` §5).
    pub fn unsupported_media_type(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            message,
        )
    }

    /// `501` — the route exists in the contract but its backing phase is not wired
    /// yet (stream/HLS are Phase 2; preview/trickplay generation is Phase 3). Kept
    /// distinct from `unavailable` so a client can tell "not built" from "try again".
    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_IMPLEMENTED, "not_implemented", message)
    }

    /// `500` — an unexpected internal failure.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            message,
        )
    }
}

/// The serialized error envelope.
#[derive(Serialize)]
struct ErrorBody<'a> {
    error: ErrorDetail<'a>,
}

#[derive(Serialize)]
struct ErrorDetail<'a> {
    code: &'a str,
    message: &'a str,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(ErrorBody {
            error: ErrorDetail {
                code: self.code,
                message: &self.message,
            },
        });
        (self.status, body).into_response()
    }
}

/// Map a persistence error onto the API error model. `NotFound` becomes a `404`;
/// a pool-exhaustion/lock error becomes a `503`; anything else is a `500` and is
/// logged (the detail is not leaked to the client).
impl From<medi_db::DbError> for ApiError {
    fn from(e: medi_db::DbError) -> Self {
        use medi_db::DbError;
        match e {
            DbError::NotFound => ApiError::not_found("resource not found"),
            DbError::Pool(_) => {
                tracing::warn!(error = %e, "db pool unavailable");
                ApiError::unavailable("database temporarily unavailable")
            }
            other => {
                tracing::error!(error = %other, "internal db error");
                ApiError::internal("internal error")
            }
        }
    }
}

/// A `spawn_blocking` join failure (the DB task panicked or was cancelled) is a `500`.
impl From<tokio::task::JoinError> for ApiError {
    fn from(e: tokio::task::JoinError) -> Self {
        tracing::error!(error = %e, "blocking db task failed to join");
        ApiError::internal("internal error")
    }
}
