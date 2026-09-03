/**
 * `StatusPage` (Task 96) — `/settings/status`, the enrichment/ingest observability panel.
 *
 * Answers "is metadata actually working, what's configured, and what needs my attention"
 * without reading container logs:
 *   - per-state title counts (matched / pending / unmatched / failed) per kind,
 *   - provider chips — most importantly `fanart ✗ (no key)`, the durable signal that title
 *     logos are off because `FANARTTV_API_KEY` is unset,
 *   - last scan / last enrichment summaries + watcher liveness,
 *   - "Run enrichment now" / "Backfill now" buttons (no waiting for the schedule),
 *   - a list of unmatched/failed titles with a per-row "Fix match" (reuses `MatchDialog`),
 *   - a list of ffprobe-failed files, so a "silently missing" title is explainable.
 */

import { useCallback, useEffect, useState } from 'react';
import {
  ApiError,
  type SystemStatus,
  type UnmatchedItem,
  type ProbeFailureItem,
} from '@medi/api-client';
import { useApi } from '../api';
import { MatchDialog } from '../components/MatchDialog';
import { Loading, ErrorState } from '../components/Status';
import { theme } from '../theme';

function ago(ts: number | null): string {
  if (ts == null) return 'never';
  const secs = Math.max(0, Math.floor(Date.now() / 1000) - ts);
  if (secs < 60) return `${secs}s ago`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;
  return `${Math.floor(secs / 86400)}d ago`;
}

function Chip({ ok, label, hint }: { ok: boolean; label: string; hint?: string }) {
  return (
    <span
      title={hint}
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 6,
        padding: '4px 10px',
        borderRadius: 999,
        fontSize: 13,
        background: ok ? 'rgba(46,160,67,0.18)' : 'rgba(220,80,80,0.18)',
        color: ok ? '#5fd67a' : '#ff8a8a',
        border: `1px solid ${ok ? 'rgba(46,160,67,0.5)' : 'rgba(220,80,80,0.5)'}`,
      }}
    >
      {ok ? '✓' : '✗'} {label}
    </span>
  );
}

function CountBar({
  title,
  c,
}: {
  title: string;
  c: SystemStatus['counts']['movies'];
}) {
  const cells: [string, number, string][] = [
    ['Matched', c.matched, '#5fd67a'],
    ['Pending', c.pending, '#e0b341'],
    ['Unmatched', c.unmatched, '#ff8a8a'],
    ['Failed', c.failed, '#c77dff'],
  ];
  return (
    <div style={{ marginBottom: 16 }}>
      <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 6 }}>
        <strong style={{ color: theme.colors.text }}>{title}</strong>
        <span style={{ color: theme.colors.textMuted }}>{c.total} total</span>
      </div>
      <div style={{ display: 'flex', gap: 12, flexWrap: 'wrap' }}>
        {cells.map(([label, n, color]) => (
          <span key={label} style={{ color: theme.colors.textMuted, fontSize: 14 }}>
            <span style={{ color, fontWeight: 600 }}>{n}</span> {label}
          </span>
        ))}
      </div>
    </div>
  );
}

const btnStyle: React.CSSProperties = {
  padding: '8px 14px',
  borderRadius: 8,
  border: `1px solid ${theme.colors.accent}`,
  background: 'transparent',
  color: theme.colors.accent,
  cursor: 'pointer',
  fontSize: 14,
};

export function StatusPage() {
  const api = useApi();
  const [status, setStatus] = useState<SystemStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [unmatched, setUnmatched] = useState<UnmatchedItem[]>([]);
  const [probeFailures, setProbeFailures] = useState<ProbeFailureItem[]>([]);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [fixMovieId, setFixMovieId] = useState<number | null>(null);
  const [fixQuery, setFixQuery] = useState<string>('');

  const refresh = useCallback(() => {
    const controller = new AbortController();
    const signal = controller.signal;
    Promise.all([
      api.status({ signal }),
      api.unmatched({ kind: 'movie', limit: 100 }, { signal }),
      api.probeFailures({ limit: 100 }, { signal }),
    ])
      .then(([s, u, p]) => {
        if (signal.aborted) return;
        setStatus(s);
        setUnmatched(u.items);
        setProbeFailures(p.items);
        setError(null);
      })
      .catch((err: unknown) => {
        if (signal.aborted) return;
        setError(err instanceof ApiError ? err.message : String(err));
      });
    return () => controller.abort();
  }, [api]);

  useEffect(() => refresh(), [refresh]);

  const run = async (kind: 'enrich' | 'backfill') => {
    if (busy) return;
    setBusy(true);
    setMsg(null);
    try {
      if (kind === 'enrich') {
        await api.enrichMetadata();
        setMsg('Enrichment pass started — pending titles are being processed.');
      } else {
        const res = await api.backfillMetadata(false);
        setMsg(res.already_running ? 'Backfill already running.' : 'Backfill started.');
      }
      // Give the pass a moment, then refresh the counts.
      setTimeout(refresh, 1500);
    } catch (err) {
      setMsg(
        err instanceof ApiError && err.isBusy
          ? 'Already running.'
          : err instanceof ApiError
            ? err.message
            : String(err),
      );
    } finally {
      setBusy(false);
    }
  };

  if (error && !status) return <ErrorState message={error} />;
  if (!status) return <Loading label="Loading status…" />;

  const { counts, providers, last_scan, last_enrichment, workers } = status;

  return (
    <div style={{ padding: 24, maxWidth: 900, margin: '0 auto', color: theme.colors.text }}>
      <h1 style={{ marginTop: 0 }}>Status</h1>

      {/* Providers — the headline signal for the "no logos" problem. */}
      <section style={{ marginBottom: 24, display: 'flex', gap: 10, flexWrap: 'wrap' }}>
        <Chip
          ok={providers.metadata.configured}
          label={`Metadata: ${providers.metadata.name ?? 'tmdb'}`}
          hint={providers.metadata.configured ? undefined : 'Set TMDB_API_KEY (or OMDB_API_KEY)'}
        />
        <Chip
          ok={providers.fanart.configured}
          label="fanart.tv (title logos)"
          hint={
            providers.fanart.configured
              ? undefined
              : 'Set FANARTTV_API_KEY to enable movie title logos'
          }
        />
        <Chip ok={workers.watcher_alive} label="Media watcher" />
        <Chip ok={status.media_dir_present} label="Media dir" />
      </section>
      {!providers.fanart.configured && (
        <p style={{ color: theme.colors.textMuted, marginTop: -12, marginBottom: 24 }}>
          Title logos are off because <code>FANARTTV_API_KEY</code> is not set. Add it to the
          container environment and run a backfill to fetch logos for already-matched movies.
        </p>
      )}

      {/* Counts. */}
      <section
        style={{
          background: theme.colors.surface,
          borderRadius: 12,
          padding: 20,
          marginBottom: 20,
        }}
      >
        <CountBar title="Movies" c={counts.movies} />
        <CountBar title="Series" c={counts.series} />
      </section>

      {/* Last runs + actions. */}
      <section
        style={{
          background: theme.colors.surface,
          borderRadius: 12,
          padding: 20,
          marginBottom: 20,
        }}
      >
        <div style={{ color: theme.colors.textMuted, fontSize: 14, lineHeight: 1.8 }}>
          <div>
            Last scan: <strong style={{ color: theme.colors.text }}>{ago(last_scan.finished_at)}</strong>
            {' — '}
            {last_scan.written} written, {last_scan.probe_failures} probe failures
          </div>
          <div>
            Last enrichment:{' '}
            <strong style={{ color: theme.colors.text }}>{ago(last_enrichment.finished_at)}</strong>
            {' — '}
            {last_enrichment.matched} matched, {last_enrichment.unmatched} unmatched,{' '}
            {last_enrichment.failed} failed
          </div>
          <div>Backfill interval: every {workers.backfill_interval_hours}h</div>
        </div>
        <div style={{ display: 'flex', gap: 12, marginTop: 16, flexWrap: 'wrap' }}>
          <button style={btnStyle} disabled={busy} onClick={() => run('enrich')}>
            Run enrichment now
          </button>
          <button style={btnStyle} disabled={busy} onClick={() => run('backfill')}>
            Backfill artwork now
          </button>
          <button style={{ ...btnStyle, borderColor: theme.colors.textMuted, color: theme.colors.textMuted }} onClick={refresh}>
            Refresh
          </button>
        </div>
        {msg && <p style={{ color: theme.colors.textMuted, marginBottom: 0 }}>{msg}</p>}
      </section>

      {/* Unmatched titles — the actionable list. */}
      <section style={{ marginBottom: 20 }}>
        <h2 style={{ fontSize: 18 }}>Unmatched movies ({unmatched.length})</h2>
        {unmatched.length === 0 ? (
          <p style={{ color: theme.colors.textMuted }}>Nothing unmatched — every movie found a match.</p>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
            {unmatched.map((u) => (
              <div
                key={u.id}
                style={{
                  display: 'flex',
                  justifyContent: 'space-between',
                  alignItems: 'center',
                  gap: 12,
                  padding: '8px 12px',
                  background: theme.colors.surface,
                  borderRadius: 8,
                }}
              >
                <div style={{ minWidth: 0 }}>
                  <div style={{ color: theme.colors.text }}>
                    {u.title}
                    {u.year != null && (
                      <span style={{ color: theme.colors.textMuted }}> ({u.year})</span>
                    )}
                    <span style={{ color: '#ff8a8a', fontSize: 12, marginLeft: 8 }}>{u.state}</span>
                  </div>
                  {u.path && (
                    <div
                      style={{
                        color: theme.colors.textMuted,
                        fontSize: 12,
                        whiteSpace: 'nowrap',
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                      }}
                    >
                      {u.path}
                    </div>
                  )}
                </div>
                <button
                  style={{ ...btnStyle, padding: '6px 10px', flexShrink: 0 }}
                  onClick={() => {
                    setFixMovieId(u.id);
                    setFixQuery(u.title);
                  }}
                >
                  Fix match
                </button>
              </div>
            ))}
          </div>
        )}
      </section>

      {/* Probe failures. */}
      {probeFailures.length > 0 && (
        <section style={{ marginBottom: 20 }}>
          <h2 style={{ fontSize: 18 }}>Files that failed to probe ({probeFailures.length})</h2>
          <p style={{ color: theme.colors.textMuted, marginTop: 0 }}>
            ffprobe could not read these — usually a corrupt/truncated file or an unsupported
            container. Replace or remove them, then rescan.
          </p>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
            {probeFailures.map((p) => (
              <div
                key={p.path}
                style={{
                  padding: '8px 12px',
                  background: theme.colors.surface,
                  borderRadius: 8,
                  fontSize: 13,
                }}
              >
                <div style={{ color: theme.colors.text, wordBreak: 'break-all' }}>{p.path}</div>
                <div style={{ color: '#ff8a8a', fontSize: 12 }}>{p.error || 'ffprobe failed'}</div>
              </div>
            ))}
          </div>
        </section>
      )}

      {fixMovieId != null && (
        <MatchDialog
          movieId={fixMovieId}
          initialQuery={fixQuery}
          onClose={() => setFixMovieId(null)}
          onMatched={() => {
            setFixMovieId(null);
            refresh();
          }}
        />
      )}
    </div>
  );
}
