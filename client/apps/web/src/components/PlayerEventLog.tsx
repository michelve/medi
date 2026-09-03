/**
 * `PlayerEventLog` — a collapsible on-screen overlay that renders the live
 * {@link PlayerDiagnostics} stream, so playback issues are visible in the browser without
 * opening devtools. Toggle it with the small "Diagnostics" button; "Copy" dumps the whole
 * log as text for a bug report.
 */

import { useEffect, useRef, useState } from 'react';
import type { DiagEvent, PlayerDiagnostics } from '../lib/playerDiagnostics';

export interface PlayerEventLogProps {
  diagnostics: PlayerDiagnostics;
  /** Open by default (handy while actively debugging a title). */
  defaultOpen?: boolean;
}

const levelColor: Record<DiagEvent['level'], string> = {
  info: '#8ab4ff',
  warn: '#e6c200',
  error: '#ff6b6b',
};

export function PlayerEventLog({ diagnostics, defaultOpen = false }: PlayerEventLogProps) {
  const [events, setEvents] = useState<readonly DiagEvent[]>(diagnostics.snapshot());
  const [open, setOpen] = useState(defaultOpen);
  const [copied, setCopied] = useState(false);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const pinnedRef = useRef(true);

  useEffect(() => diagnostics.subscribe(setEvents), [diagnostics]);

  // Auto-scroll to the newest event unless the user scrolled up to read history.
  useEffect(() => {
    const el = scrollRef.current;
    if (el && pinnedRef.current) el.scrollTop = el.scrollHeight;
  }, [events, open]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
  };

  const copy = () => {
    void navigator.clipboard?.writeText(diagnostics.toText()).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      },
      () => undefined,
    );
  };

  const errorCount = events.filter((e) => e.level === 'error').length;
  const warnCount = events.filter((e) => e.level === 'warn').length;

  return (
    <div style={wrap}>
      <div style={bar}>
        <button type="button" onClick={() => setOpen((v) => !v)} style={toggleBtn}>
          {open ? '▾' : '▸'} Diagnostics
          <span style={{ opacity: 0.7, marginLeft: 8 }}>{events.length} events</span>
          {errorCount > 0 && <span style={badge('#b00020')}>{errorCount} err</span>}
          {warnCount > 0 && <span style={badge('#8a6d00')}>{warnCount} warn</span>}
        </button>
        {open && (
          <button type="button" onClick={copy} style={copyBtn}>
            {copied ? 'Copied ✓' : 'Copy log'}
          </button>
        )}
      </div>
      {open && (
        <div ref={scrollRef} onScroll={onScroll} style={logBox}>
          {events.length === 0 ? (
            <div style={{ opacity: 0.6, padding: 8 }}>No events yet…</div>
          ) : (
            events.map((e) => <Row key={e.id} e={e} />)
          )}
        </div>
      )}
    </div>
  );
}

function Row({ e }: { e: DiagEvent }) {
  const [expanded, setExpanded] = useState(false);
  const hasDetail = e.detail !== undefined;
  return (
    <div style={{ borderBottom: '1px solid rgba(255,255,255,0.06)', padding: '3px 8px' }}>
      <div
        style={{ display: 'flex', gap: 8, cursor: hasDetail ? 'pointer' : 'default' }}
        onClick={() => hasDetail && setExpanded((v) => !v)}
      >
        <span style={{ color: '#7a7a86', minWidth: 62, textAlign: 'right' }}>+{e.t}ms</span>
        <span style={{ color: levelColor[e.level], minWidth: 44 }}>{e.scope}</span>
        <span style={{ color: '#e8e8ea', flex: 1 }}>
          {e.event}
          {hasDetail && <span style={{ opacity: 0.5, marginLeft: 6 }}>{expanded ? '▾' : '▸'}</span>}
        </span>
      </div>
      {hasDetail && expanded && (
        <pre style={detailPre}>{prettyDetail(e.detail)}</pre>
      )}
    </div>
  );
}

function prettyDetail(detail: unknown): string {
  try {
    return typeof detail === 'string' ? detail : JSON.stringify(detail, null, 2);
  } catch {
    return String(detail);
  }
}

function badge(bg: string): React.CSSProperties {
  return {
    background: bg,
    color: '#fff',
    borderRadius: 4,
    padding: '0 6px',
    marginLeft: 8,
    fontSize: 11,
  };
}

const wrap: React.CSSProperties = {
  marginTop: 8,
  fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
  fontSize: 12,
  color: '#e8e8ea',
  background: '#111116',
  border: '1px solid rgba(255,255,255,0.1)',
  borderRadius: 8,
  overflow: 'hidden',
};

const bar: React.CSSProperties = {
  display: 'flex',
  justifyContent: 'space-between',
  alignItems: 'center',
  padding: '6px 8px',
  background: '#17171d',
};

const toggleBtn: React.CSSProperties = {
  background: 'transparent',
  border: 'none',
  color: '#e8e8ea',
  cursor: 'pointer',
  fontSize: 12,
  fontFamily: 'inherit',
  display: 'flex',
  alignItems: 'center',
};

const copyBtn: React.CSSProperties = {
  background: '#0a84ff',
  border: 'none',
  color: '#fff',
  cursor: 'pointer',
  borderRadius: 6,
  padding: '4px 10px',
  fontSize: 12,
  fontFamily: 'inherit',
};

const logBox: React.CSSProperties = {
  maxHeight: 260,
  overflowY: 'auto',
  overflowX: 'hidden',
};

const detailPre: React.CSSProperties = {
  margin: '4px 0 4px 70px',
  padding: 8,
  background: '#0b0b0f',
  borderRadius: 6,
  whiteSpace: 'pre-wrap',
  wordBreak: 'break-word',
  color: '#b9b9c3',
  maxHeight: 200,
  overflow: 'auto',
};
