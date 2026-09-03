/**
 * `AboutSection` (Task 91, Apple TV-style refresh) — the "About" block at the bottom of a
 * movie detail page.
 *
 * A responsive multi-column layout echoing tvOS's movie "About" panel, built from the
 * metadata medi actually has: a synopsis card (title + genre + expandable overview), an
 * Information column (release year, run time, video format, file), a Languages column
 * (audio + subtitle tracks derived from the best file's streams), and an Accessibility note
 * when closed captions are present. Fields with no data are omitted rather than shown blank,
 * so the panel degrades cleanly for unprobed / unmatched titles.
 */

import type { MovieDetail } from '@medi/api-client';
import { detail } from '../theme';
import { ExpandableOverview } from './ExpandableOverview';
import {
  formatRuntime,
  formatBytes,
  formatResolution,
  formatToken,
  languageName,
  channelLabel,
} from '../lib/format';
import { hdrLabel } from './HdrBadge';
import { pickBestFile } from '../lib/bestFile';

/**
 * A `label` / `value` stack used by the Information block. Figma: 16px Inter Medium label
 * in white over a 16px Regular value, 4px apart.
 */
function Field({ label, value }: { label: string; value: string }) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
      <div style={{ fontSize: 16, fontWeight: 500, color: detail.text.primary, lineHeight: '24px' }}>
        {label}
      </div>
      <div style={{ fontSize: 16, fontWeight: 400, color: detail.text.primary, lineHeight: '24px' }}>
        {value}
      </div>
    </div>
  );
}

/** Unique, in-order display list from a stream's languages (skips empties/dupes). */
function languages(codes: (string | null)[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const c of codes) {
    const name = languageName(c);
    if (name && !seen.has(name)) {
      seen.add(name);
      out.push(name);
    }
  }
  return out;
}

export function AboutSection({ movie }: { movie: MovieDetail }) {
  // Describe the best copy (a movie may carry several resolutions).
  const file = pickBestFile(movie.media_files ?? []);
  const genreText = (movie.genres ?? []).map((g) => g.name).join(', ');

  // --- Information column ---------------------------------------------------
  const info: Array<{ label: string; value: string }> = [];
  if (genreText) info.push({ label: 'Genre', value: genreText });
  if (movie.year != null) info.push({ label: 'Released', value: String(movie.year) });
  const runtime = formatRuntime(file?.duration_ms);
  if (runtime) info.push({ label: 'Run Time', value: runtime });
  const video = [
    formatToken(file?.video_codec),
    formatResolution(file?.width, file?.height),
    hdrLabel(file?.hdr_type),
  ]
    .filter(Boolean)
    .join(' · ');
  if (video) info.push({ label: 'Video', value: video });
  const fileLine = [formatToken(file?.container), formatBytes(file?.size_bytes)]
    .filter(Boolean)
    .join(' · ');
  if (fileLine) info.push({ label: 'File', value: fileLine });

  // --- Languages column -----------------------------------------------------
  const audioTracks =
    file?.audio_streams?.map((a) => {
      const parts = [
        languageName(a.language) ?? 'Audio',
        formatToken(a.codec),
        channelLabel(a.channels),
      ].filter(Boolean);
      return parts.join(', ');
    }) ?? [];
  const subtitleLangs = languages(file?.subtitle_streams?.map((s) => s.language) ?? []);
  // Fold the language tracks into the Information list (Figma has one Information block;
  // no data is dropped — Audio/Subtitles just become additional fields).
  if (audioTracks.length > 0) info.push({ label: 'Audio', value: audioTracks.join(' · ') });
  if (subtitleLangs.length > 0) info.push({ label: 'Subtitles', value: subtitleLangs.join(', ') });

  // Split the Information fields across two balanced sub-columns, as in the Figma footer.
  const mid = Math.ceil(info.length / 2);
  const infoCols = [info.slice(0, mid), info.slice(mid)];

  return (
    // Figma "Details Footer": two columns side by side — synopsis on the left, an
    // Information block on the right — wrapping to a single column when narrow.
    <section
      style={{
        display: 'grid',
        gridTemplateColumns: 'repeat(auto-fit, minmax(320px, 1fr))',
        gap: 28,
        alignItems: 'start',
      }}
    >
      {/* Left: title (20px medium) · genre (55% white) · synopsis (16px). */}
      <div style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
        <div style={{ fontSize: 20, fontWeight: 500, color: detail.text.primary, lineHeight: '24px' }}>
          {movie.title}
        </div>
        {genreText && (
          <div style={{ fontSize: 16, color: detail.text.tertiary, lineHeight: '24px' }}>
            {genreText}
          </div>
        )}
        {movie.overview ? (
          <ExpandableOverview text={movie.overview} lines={4} size={16} />
        ) : (
          <div style={{ fontSize: 16, color: detail.text.secondary }}>No synopsis available.</div>
        )}
      </div>

      {/* Right: "Information" over two balanced label/value sub-columns. */}
      {info.length > 0 && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          <div style={{ fontSize: 16, fontWeight: 600, color: detail.text.primary, lineHeight: '24px' }}>
            Information
          </div>
          <div style={{ display: 'flex', gap: 24, alignItems: 'flex-start' }}>
            {infoCols.map((col, i) => (
              <div
                key={i}
                style={{
                  flex: '1 1 0',
                  minWidth: 0,
                  display: 'flex',
                  flexDirection: 'column',
                  gap: 16,
                }}
              >
                {col.map((f) => (
                  <Field key={f.label} label={f.label} value={f.value} />
                ))}
              </div>
            ))}
          </div>
        </div>
      )}
    </section>
  );
}
