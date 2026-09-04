/**
 * Subtitle/audio track selection: labels, auto-selection, and cross-title persistence
 * (`docs/.tasks/99` A2/C3). Pure helpers over the `GET /api/files/:id` track shapes.
 *
 * Persistence remembers the viewer's last choice and carries it FORWARD across episodes by
 * scoring candidate tracks (language/title/index), mirroring Jellyfin's `rankStreamType` — so
 * picking "English SDH" on episode 1 keeps it on episode 2 even when stream indices differ.
 */

import type { FileAudioTrack, FileSubtitleTrack } from '@medi/api-client';

/** A stable id for addressing a subtitle track in the player + URL (`ext<id>` for external,
 * else the embedded `stream_index`). Matches the `/api/subtitles/:id/:index` index form. */
export function subtitleTrackId(t: FileSubtitleTrack): string {
  return t.external ? `ext${t.id}` : String(t.stream_index ?? t.id);
}

/** Language display name (best-effort) from an ISO-639 tag, falling back to the raw tag. */
export function languageLabel(code: string | undefined): string | null {
  if (!code) return null;
  try {
    const dn = new Intl.DisplayNames([navigator.language || 'en'], { type: 'language' });
    return dn.of(code) ?? code;
  } catch {
    return code;
  }
}

/** Heuristic: does this track look like SDH / hearing-impaired? (title cue only — the backend
 * has no dedicated flag yet). */
function looksSdh(t: FileSubtitleTrack): boolean {
  const s = `${t.title ?? ''}`.toLowerCase();
  return s.includes('sdh') || s.includes('hearing') || s.includes('cc');
}

/** A caption menu label: name + badges, e.g. `English · Forced`, `English SDH · Default`. */
export function subtitleLabel(t: FileSubtitleTrack, index: number): string {
  const name = t.title || languageLabel(t.language) || `Track ${index + 1}`;
  const badges: string[] = [];
  if (t.is_forced) badges.push('Forced');
  if (t.is_default) badges.push('Default');
  if (!t.title && looksSdh(t)) badges.push('SDH');
  const suffix = t.format === 'image' ? ' (burn-in)' : '';
  return badges.length ? `${name} · ${badges.join(', ')}${suffix}` : `${name}${suffix}`;
}

// --- Persistence (localStorage) --------------------------------------------------------------

/** What we remember about a viewer's last subtitle choice, to carry forward across titles. */
export interface RememberedSubtitle {
  /** `null` = the viewer explicitly chose Off. */
  language: string | null;
  title: string | null;
  forced: boolean;
  /** `true` when the viewer turned captions off entirely. */
  off: boolean;
}

const SUB_KEY = 'medi.player.subtitle';

/** Persist the viewer's subtitle choice (safe if storage is unavailable). */
export function rememberSubtitle(choice: RememberedSubtitle): void {
  try {
    localStorage.setItem(SUB_KEY, JSON.stringify(choice));
  } catch {
    // Private mode / storage disabled — non-fatal, we just won't remember.
  }
}

/** Read the remembered subtitle choice, or `null` when none / unreadable. */
export function readRememberedSubtitle(): RememberedSubtitle | null {
  try {
    const raw = localStorage.getItem(SUB_KEY);
    return raw ? (JSON.parse(raw) as RememberedSubtitle) : null;
  } catch {
    return null;
  }
}

// --- Auto-selection --------------------------------------------------------------------------

/** Score how well a candidate track matches a remembered choice (higher = better), mirroring
 * Jellyfin's rankStreamType: +2 same language, +2 same title, +1 same forced flag. */
function scoreMatch(t: FileSubtitleTrack, want: RememberedSubtitle): number {
  let score = 0;
  if (want.language && t.language && t.language === want.language) score += 2;
  if (want.title && t.title && t.title === want.title) score += 2;
  if (t.is_forced === want.forced) score += 1;
  return score;
}

/**
 * Choose which subtitle track to show on first attach (`docs/.tasks/99` A2/C3).
 *
 * 1. If the viewer has a remembered choice: honor Off, else pick the best-scoring track
 *    (score ≥ 3) — this is the "carry my track across episodes" behavior.
 * 2. Else fall back to the file's own default/forced track.
 * 3. Else no captions.
 *
 * Returns the chosen track, or `null` for Off (the caller derives its id/render path).
 */
export function autoSelectSubtitle(
  tracks: FileSubtitleTrack[],
  remembered: RememberedSubtitle | null,
): FileSubtitleTrack | null {
  // Only text tracks are selectable via native <track>; image tracks need burn-in (Phase 5).
  const selectable = tracks.filter((t) => t.format !== 'image');
  if (selectable.length === 0) return null;

  if (remembered) {
    if (remembered.off) return null;
    let best: FileSubtitleTrack | null = null;
    let bestScore = 0;
    for (const t of selectable) {
      const s = scoreMatch(t, remembered);
      if (s > bestScore) {
        best = t;
        bestScore = s;
      }
    }
    if (best && bestScore >= 3) return best;
  }

  // Fall back to the file's default/forced track.
  return selectable.find((t) => t.is_default) ?? selectable.find((t) => t.is_forced) ?? null;
}

// --- Audio track persistence + auto-selection (`docs/.tasks/99` C3) -------------------------

/** What we remember about a viewer's last audio choice, to carry forward across titles. */
export interface RememberedAudio {
  language: string | null;
  title: string | null;
}

const AUDIO_KEY = 'medi.player.audio';

/** Persist the viewer's audio choice (safe if storage is unavailable). */
export function rememberAudio(choice: RememberedAudio): void {
  try {
    localStorage.setItem(AUDIO_KEY, JSON.stringify(choice));
  } catch {
    /* storage disabled — non-fatal */
  }
}

/** Read the remembered audio choice, or `null` when none / unreadable. */
export function readRememberedAudio(): RememberedAudio | null {
  try {
    const raw = localStorage.getItem(AUDIO_KEY);
    return raw ? (JSON.parse(raw) as RememberedAudio) : null;
  } catch {
    return null;
  }
}

function scoreAudioMatch(t: FileAudioTrack, want: RememberedAudio): number {
  let score = 0;
  if (want.language && t.language && t.language === want.language) score += 2;
  if (want.title && t.title && t.title === want.title) score += 2;
  return score;
}

/**
 * Choose which audio track to select on load: the best-scoring match for the remembered choice
 * (score ≥ 2), else the file's default track, else the first — mirroring the subtitle logic.
 * Returns the track's `stream_index`, or `undefined` to leave the server default.
 */
export function autoSelectAudio(
  tracks: FileAudioTrack[],
  remembered: RememberedAudio | null,
): number | undefined {
  if (tracks.length === 0) return undefined;
  if (remembered) {
    let best: FileAudioTrack | null = null;
    let bestScore = 0;
    for (const t of tracks) {
      const s = scoreAudioMatch(t, remembered);
      if (s > bestScore) {
        best = t;
        bestScore = s;
      }
    }
    if (best && bestScore >= 2) return best.stream_index;
  }
  // No remembered match → leave the server default (don't force a track the server didn't pick).
  return undefined;
}
