//! Opaque keyset pagination cursors (`docs/.tasks/02-api-contract.md` §sub-task 2).
//!
//! The client treats `next_cursor` as an opaque token; internally it is
//! URL-safe base64 of a tiny JSON object carrying the last row's ordering key
//! (`{ "s": <sort_value>, "k": <kind_tag>, "i": <id> }`). Encoding the *values*
//! rather than an offset is what keeps pagination keyset-based — the next query
//! resumes immediately after the encoded row with no `OFFSET` walk.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};

use medi_db::queries::LibraryCursor;

use crate::error::ApiError;

/// Wire form of a [`LibraryCursor`]. Field names are terse to keep the token short.
#[derive(Serialize, Deserialize)]
struct CursorWire {
    /// sort_value — `sort_title`, or `added_at` rendered as text.
    s: String,
    /// kind_tag — 0 = movie, 1 = series.
    k: i64,
    /// id.
    i: i64,
}

/// Encode a [`LibraryCursor`] into the opaque `next_cursor` string.
pub fn encode(cursor: &LibraryCursor) -> String {
    let wire = CursorWire {
        s: cursor.sort_value.clone(),
        k: cursor.kind_tag,
        i: cursor.id,
    };
    // serde_json::to_vec on this fixed shape cannot fail; fall back to empty on the
    // impossible error rather than panicking in a request handler.
    let json = serde_json::to_vec(&wire).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(json)
}

/// Decode an opaque `cursor` query param back into a [`LibraryCursor`].
///
/// A malformed token is a client error, not a server fault: it maps to `400`.
pub fn decode(token: &str) -> Result<LibraryCursor, ApiError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| ApiError::bad_request("invalid cursor encoding"))?;
    let wire: CursorWire =
        serde_json::from_slice(&bytes).map_err(|_| ApiError::bad_request("invalid cursor"))?;
    Ok(LibraryCursor {
        sort_value: wire.s,
        kind_tag: wire.k,
        id: wire.i,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let c = LibraryCursor {
            sort_value: "blade runner 2049".to_string(),
            kind_tag: 0,
            id: 12,
        };
        let token = encode(&c);
        // URL-safe, no padding: no '+', '/', or '=' that would need escaping in a query.
        assert!(!token.contains(['+', '/', '=']));
        assert_eq!(decode(&token).unwrap(), c);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode("!!!not base64!!!").is_err());
        // valid base64 but not our JSON shape.
        let not_ours = URL_SAFE_NO_PAD.encode(b"{\"hello\":1}");
        assert!(decode(&not_ours).is_err());
    }
}
