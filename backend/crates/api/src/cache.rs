//! moka LRU response cache for catalog GETs
//! (`docs/.tasks/02-api-contract.md` §Caching & error model, sub-task 3).
//!
//! Catalog responses are pure functions of the database, so we cache the serialized
//! JSON body keyed by `path+query`, tag each with a weak `ETag` (a hash of the body),
//! honor `If-None-Match` → `304`, and let ingest invalidate the whole namespace on a
//! write. The stored value is the finished bytes + its ETag, so a cache hit skips both
//! the DB round trip and re-serialization.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use moka::future::Cache;

use crate::error::ApiResult;

/// A cached catalog response: the serialized JSON body and its ETag value.
#[derive(Clone)]
struct CachedResponse {
    body: Arc<[u8]>,
    etag: Arc<str>,
}

/// The catalog response cache. Cheap to clone (shares one `moka::Cache`).
#[derive(Clone)]
pub struct ResponseCache {
    inner: Cache<String, CachedResponse>,
}

impl ResponseCache {
    /// Build a cache holding up to `capacity` distinct catalog responses.
    ///
    /// Entries are small (a page of cards or one detail doc) and there are few
    /// distinct catalog URLs, so a modest capacity covers the working set; the LRU
    /// evicts the coldest keys past it.
    pub fn new(capacity: u64) -> Self {
        Self {
            inner: Cache::new(capacity),
        }
    }

    /// Return the cached response for `key`, or compute it with `compute`, store it,
    /// and return it — then render it against the request headers so a matching
    /// `If-None-Match` yields `304 Not Modified` instead of the body.
    ///
    /// `compute` runs only on a miss. It returns the JSON body bytes (already
    /// serialized); the ETag is derived here so callers never manage it.
    pub async fn get_or_render<F, Fut>(
        &self,
        key: String,
        req_headers: &HeaderMap,
        compute: F,
    ) -> ApiResult<Response>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ApiResult<Vec<u8>>>,
    {
        if let Some(hit) = self.inner.get(&key).await {
            return Ok(render(&hit, req_headers));
        }

        let body = compute().await?;
        let etag = etag_for(&body);
        let entry = CachedResponse {
            body: Arc::from(body.into_boxed_slice()),
            etag: Arc::from(etag.as_str()),
        };
        self.inner.insert(key, entry.clone()).await;
        Ok(render(&entry, req_headers))
    }

    /// Drop every cached catalog response. Called by the ingest worker after it
    /// writes new rows, so the next catalog GET reflects the change (sub-task on the
    /// `10` phase doc: "Invalidate cache on ingest write").
    ///
    /// `moka`'s invalidation is synchronous to schedule (entries are dropped lazily),
    /// so this is not `async`.
    pub fn invalidate_all(&self) {
        self.inner.invalidate_all();
    }
}

/// Render a cached entry into a response, honoring `If-None-Match`.
fn render(entry: &CachedResponse, req_headers: &HeaderMap) -> Response {
    if request_matches(req_headers, &entry.etag) {
        return not_modified(&entry.etag);
    }

    let mut resp = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ETAG, entry.etag.as_ref())
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(entry.body.to_vec()))
        .expect("valid catalog response");
    // A weak-cache hint for intermediaries; the ETag is the real validator.
    resp.headers_mut()
        .insert(header::VARY, header::HeaderValue::from_static("If-None-Match"));
    resp
}

/// A bodyless `304` that still carries the ETag, per HTTP caching rules.
fn not_modified(etag: &str) -> Response {
    let mut resp = StatusCode::NOT_MODIFIED.into_response();
    if let Ok(v) = header::HeaderValue::from_str(etag) {
        resp.headers_mut().insert(header::ETAG, v);
    }
    resp
}

/// Does the request's `If-None-Match` list this ETag (or `*`)? Compares against the
/// quoted tag we emit, and tolerates a `W/` weak prefix on either side.
fn request_matches(req_headers: &HeaderMap, etag: &str) -> bool {
    let Some(inm) = req_headers.get(header::IF_NONE_MATCH) else {
        return false;
    };
    let Ok(inm) = inm.to_str() else {
        return false;
    };
    inm.split(',').any(|candidate| {
        let c = candidate.trim();
        c == "*" || strip_weak(c) == strip_weak(etag)
    })
}

fn strip_weak(tag: &str) -> &str {
    tag.strip_prefix("W/").unwrap_or(tag).trim()
}

/// Compute a quoted ETag from a body: a hash of the bytes. Not cryptographic — an
/// ETag only needs to change when the body does, which a content hash guarantees.
fn etag_for(body: &[u8]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    body.hash(&mut hasher);
    format!("\"{:016x}\"", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_is_stable_and_content_addressed() {
        let a = etag_for(b"hello");
        assert_eq!(a, etag_for(b"hello"));
        assert_ne!(a, etag_for(b"world"));
        assert!(a.starts_with('"') && a.ends_with('"'));
    }

    #[test]
    fn if_none_match_star_and_exact_match() {
        let etag = etag_for(b"body");
        let mut h = HeaderMap::new();
        h.insert(header::IF_NONE_MATCH, "*".parse().unwrap());
        assert!(request_matches(&h, &etag));

        let mut h2 = HeaderMap::new();
        h2.insert(header::IF_NONE_MATCH, etag.parse().unwrap());
        assert!(request_matches(&h2, &etag));

        let mut h3 = HeaderMap::new();
        h3.insert(header::IF_NONE_MATCH, "\"deadbeef\"".parse().unwrap());
        assert!(!request_matches(&h3, &etag));
    }
}
