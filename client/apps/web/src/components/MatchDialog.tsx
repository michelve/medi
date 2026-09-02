/**
 * `MatchDialog` (Task 82) — fix a movie's metadata match.
 *
 * A modal opened from the movie detail page. It can force a re-enrich
 * (`client.refreshMovie`), list candidate provider matches (`client.movieMatches`, with an
 * optional corrected search term), and pin a chosen candidate (`client.matchMovie`). On a
 * successful pin/refresh it calls `onMatched` so the page re-fetches the movie and the new
 * poster/overview appear.
 */

import { useCallback, useEffect, useState } from 'react';
import { ApiError, type MatchCandidate } from '@medi/api-client';
import { useApi } from '../api';
import { theme } from '../theme';

export interface MatchDialogProps {
  movieId: number;
  /** Prefill the candidate search with the movie's current title. */
  initialQuery?: string;
  onClose: () => void;
  /** Called after a successful refresh or pin so the page can re-fetch the movie. */
  onMatched: () => void;
}

export function MatchDialog({ movieId, initialQuery, onClose, onMatched }: MatchDialogProps) {
  const api = useApi();
  const [query, setQuery] = useState(initialQuery ?? '');
  const [candidates, setCandidates] = useState<MatchCandidate[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const loadCandidates = useCallback(
    async (q: string, signal?: AbortSignal) => {
      setLoading(true);
      setError(null);
      try {
        const res = await api.movieMatches(movieId, q || undefined, { signal });
        if (!signal?.aborted) setCandidates(res.candidates);
      } catch (err) {
        if (signal?.aborted) return;
        // 501 (no provider configured) or any error → show the message, no candidates.
        setError(err instanceof ApiError ? err.message : String(err));
        setCandidates([]);
      } finally {
        if (!signal?.aborted) setLoading(false);
      }
    },
    [api, movieId],
  );

  // Initial candidate load.
  useEffect(() => {
    const controller = new AbortController();
    void loadCandidates(initialQuery ?? '', controller.signal);
    return () => controller.abort();
  }, [loadCandidates, initialQuery]);

  const pin = async (providerId: string) => {
    if (busyId) return;
    setBusyId(providerId);
    setError(null);
    try {
      await api.matchMovie(movieId, providerId);
      onMatched();
      onClose();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusyId(null);
    }
  };

  const refresh = async () => {
    setBusyId('__refresh__');
    setError(null);
    try {
      await api.refreshMovie(movieId);
      onMatched();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Fix metadata match"
      onClick={onClose}
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 100,
        background: 'rgba(0,0,0,0.6)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 24,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 'min(560px, 100%)',
          maxHeight: '80vh',
          overflow: 'auto',
          background: theme.colors.surface,
          borderRadius: 12,
          padding: 20,
          display: 'grid',
          gap: 14,
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <h2 style={{ fontSize: 18, margin: 0, flex: 1 }}>Fix match</h2>
          <button type="button" onClick={onClose} aria-label="Close" style={closeBtn}>
            ✕
          </button>
        </div>

        <div style={{ display: 'flex', gap: 8 }}>
          <input
            className="medi-search-input"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && loadCandidates(query)}
            placeholder="Search title…"
            aria-label="Search term"
            style={{ ...inputStyle, flex: 1 }}
          />
          <button
            type="button"
            onClick={() => loadCandidates(query)}
            disabled={loading}
            style={primaryBtn}
          >
            Search
          </button>
        </div>

        {loading && <p style={{ color: theme.colors.textMuted, margin: 0 }}>Searching…</p>}
        {error && <p style={{ color: '#ff6b6b', margin: 0, fontSize: 13 }}>{error}</p>}

        {candidates && candidates.length > 0 && (
          <ul style={{ listStyle: 'none', padding: 0, margin: 0, display: 'grid', gap: 8 }}>
            {candidates.map((c) => (
              <li
                key={c.provider_id}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 12,
                  padding: '10px 12px',
                  borderRadius: 8,
                  background: theme.colors.background,
                }}
              >
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontSize: 14, color: theme.colors.text }}>
                    {c.title}
                    {c.year != null && (
                      <span style={{ color: theme.colors.textMuted }}> ({c.year})</span>
                    )}
                  </div>
                  <div style={{ fontSize: 12, color: theme.colors.textMuted }}>
                    match {Math.round(c.score * 100)}%
                  </div>
                </div>
                <button
                  type="button"
                  onClick={() => pin(c.provider_id)}
                  disabled={busyId !== null}
                  style={primaryBtn}
                >
                  {busyId === c.provider_id ? 'Pinning…' : 'Use this'}
                </button>
              </li>
            ))}
          </ul>
        )}
        {candidates && candidates.length === 0 && !loading && !error && (
          <p style={{ color: theme.colors.textMuted, margin: 0, fontSize: 13 }}>
            No candidates. Try a different search term.
          </p>
        )}

        <div style={{ borderTop: `1px solid ${theme.colors.background}`, paddingTop: 12 }}>
          <button
            type="button"
            onClick={refresh}
            disabled={busyId !== null}
            style={{ ...primaryBtn, background: 'transparent', color: theme.colors.accent, border: `1px solid ${theme.colors.accent}` }}
          >
            {busyId === '__refresh__' ? 'Refreshing…' : 'Re-run auto-match'}
          </button>
        </div>
      </div>
    </div>
  );
}

const inputStyle: React.CSSProperties = {
  padding: '8px 12px',
  borderRadius: 8,
  border: `1px solid ${theme.colors.background}`,
  background: theme.colors.background,
  color: theme.colors.text,
  fontSize: 14,
  outline: 'none',
};
const primaryBtn: React.CSSProperties = {
  padding: '8px 14px',
  borderRadius: 8,
  border: 'none',
  fontSize: 13,
  fontWeight: 600,
  cursor: 'pointer',
  color: '#fff',
  background: theme.colors.accent,
  whiteSpace: 'nowrap',
};
const closeBtn: React.CSSProperties = {
  border: 'none',
  background: 'transparent',
  color: theme.colors.textMuted,
  cursor: 'pointer',
  fontSize: 16,
};
