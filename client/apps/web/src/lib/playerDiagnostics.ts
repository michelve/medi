/**
 * Player diagnostics — a tiny structured event log for browser playback.
 *
 * Playback failures in the browser are hard to see: the `<video>` element and hls.js each
 * surface state through different callbacks, and a black screen tells you nothing. This
 * collects every relevant event (stream decision, hls.js lifecycle + errors, `<video>` state
 * transitions, network requests) into one timestamped stream that is BOTH:
 *   - logged to the browser console (grouped, with a `[player]` prefix), and
 *   - kept in an in-memory ring buffer the on-screen `PlayerEventLog` overlay renders live.
 *
 * It is deliberately dependency-free and side-effect-light so it can wrap any playback path.
 */

export type DiagLevel = 'info' | 'warn' | 'error';

export interface DiagEvent {
  /** Monotonic id for React keys. */
  id: number;
  /** ms since this session's first event — easier to read than wall-clock. */
  t: number;
  level: DiagLevel;
  /** Short source tag: 'decision' | 'hls' | 'video' | 'net' | 'player'. */
  scope: string;
  /** Human-readable event name. */
  event: string;
  /** Optional structured detail (shown expandable in the overlay, dir()'d in console). */
  detail?: unknown;
}

type Listener = (events: readonly DiagEvent[]) => void;

const MAX_EVENTS = 400;

/**
 * One diagnostics channel. A `VideoPlayer` creates a fresh one per mount so a re-mount /
 * file switch starts a clean log. `PlayerEventLog` subscribes to render it.
 */
export class PlayerDiagnostics {
  private events: DiagEvent[] = [];
  private listeners = new Set<Listener>();
  private seq = 0;
  private readonly start = performance.now();
  /** Toggle console output (kept on — it's the durable record if the overlay is closed). */
  consoleEnabled = true;

  log(level: DiagLevel, scope: string, event: string, detail?: unknown): void {
    const e: DiagEvent = {
      id: this.seq++,
      t: Math.round(performance.now() - this.start),
      level,
      scope,
      event,
      detail,
    };
    this.events.push(e);
    if (this.events.length > MAX_EVENTS) this.events.splice(0, this.events.length - MAX_EVENTS);
    this.emit();
    if (this.consoleEnabled) this.toConsole(e);
  }

  info(scope: string, event: string, detail?: unknown): void {
    this.log('info', scope, event, detail);
  }
  warn(scope: string, event: string, detail?: unknown): void {
    this.log('warn', scope, event, detail);
  }
  error(scope: string, event: string, detail?: unknown): void {
    this.log('error', scope, event, detail);
  }

  snapshot(): readonly DiagEvent[] {
    return this.events;
  }

  subscribe(fn: Listener): () => void {
    this.listeners.add(fn);
    fn(this.events);
    return () => this.listeners.delete(fn);
  }

  /** Copyable plain-text dump for pasting into a bug report. */
  toText(): string {
    return this.events
      .map((e) => {
        const d = e.detail === undefined ? '' : `  ${safeStringify(e.detail)}`;
        return `+${String(e.t).padStart(6)}ms [${e.level}] ${e.scope}/${e.event}${d}`;
      })
      .join('\n');
  }

  private emit(): void {
    const snap = this.events.slice();
    for (const l of this.listeners) l(snap);
  }

  private toConsole(e: DiagEvent): void {
    const label = `%c[player]%c +${e.t}ms ${e.scope}/${e.event}`;
    const tag =
      e.level === 'error'
        ? 'background:#b00020;color:#fff;padding:1px 4px;border-radius:3px'
        : e.level === 'warn'
          ? 'background:#8a6d00;color:#fff;padding:1px 4px;border-radius:3px'
          : 'background:#0a84ff;color:#fff;padding:1px 4px;border-radius:3px';
    const fn = e.level === 'error' ? console.error : e.level === 'warn' ? console.warn : console.log;
    if (e.detail === undefined) fn(label, tag, 'color:inherit');
    else fn(label, tag, 'color:inherit', e.detail);
  }
}

function safeStringify(v: unknown): string {
  try {
    return typeof v === 'string' ? v : JSON.stringify(v);
  } catch {
    return String(v);
  }
}

/** Map a numeric `MediaError.code` to its name for readable logs. */
export function mediaErrorName(code: number | undefined): string {
  switch (code) {
    case 1:
      return 'MEDIA_ERR_ABORTED';
    case 2:
      return 'MEDIA_ERR_NETWORK';
    case 3:
      return 'MEDIA_ERR_DECODE';
    case 4:
      return 'MEDIA_ERR_SRC_NOT_SUPPORTED';
    default:
      return `code ${code ?? '?'}`;
  }
}

/** Map `HTMLMediaElement.readyState` to its name. */
export function readyStateName(rs: number): string {
  return (
    ['HAVE_NOTHING', 'HAVE_METADATA', 'HAVE_CURRENT_DATA', 'HAVE_FUTURE_DATA', 'HAVE_ENOUGH_DATA'][
      rs
    ] ?? `readyState ${rs}`
  );
}

/** Map `HTMLMediaElement.networkState` to its name. */
export function networkStateName(ns: number): string {
  return (
    ['NETWORK_EMPTY', 'NETWORK_IDLE', 'NETWORK_LOADING', 'NETWORK_NO_SOURCE'][ns] ??
    `networkState ${ns}`
  );
}
