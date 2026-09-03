/**
 * `PlaybackBadges` (Task 91 detail banner) — the small quality pills shown on a movie's
 * banner: video format (Dolby Vision / HDR10+ / HDR10 / HLG, plus a resolution label like
 * 4K), immersive audio (Dolby Atmos / DTS:X), and a CC pill when any file carries subtitles.
 *
 * The values are summarised across *all* the title's media files so the banner advertises
 * the best available format (a title may have several files of differing quality). Renders
 * nothing when nothing noteworthy is present.
 */

import type { MediaFile, HdrTier, ImmersiveAudio } from '@medi/api-client';
import { detail } from '../theme';
import { hdrLabel } from './HdrBadge';
import { resolutionLabel } from '../lib/format';
import { HDR_RANK } from '../lib/bestFile';

/** Marketing labels for the immersive-audio markers. */
const IMMERSIVE_LABELS: Record<Exclude<ImmersiveAudio, 'none'>, string> = {
  dolby_atmos: 'Dolby Atmos',
  dts_x: 'DTS:X',
};

function Pill({ children }: { children: React.ReactNode }) {
  // Figma content badges: a 20% white fill, 14px semibold 80%-white text, 16px radius with
  // 4px padding. Single-glyph badges (4K/DV/CC) sit in a square-ish pill; longer labels
  // (Dolby Atmos) stretch the pill horizontally.
  return (
    <span
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        minWidth: 22,
        padding: 4,
        borderRadius: 16,
        fontSize: 14,
        fontWeight: 600,
        lineHeight: '14px',
        color: detail.text.secondary,
        background: detail.badgeBg,
        whiteSpace: 'nowrap',
      }}
    >
      {children}
    </span>
  );
}

export function PlaybackBadges({ files }: { files: MediaFile[] }) {
  // Strongest HDR tier across files.
  let hdr: HdrTier | undefined;
  let hdrRank = 0;
  // Best resolution across files (by width — the marketing-class axis).
  let maxWidth = 0;
  let maxHeight = 0;
  // Immersive-audio formats present anywhere.
  const immersive = new Set<Exclude<ImmersiveAudio, 'none'>>();
  // Whether any file carries a subtitle track.
  let hasCaptions = false;

  for (const f of files) {
    if (f.hdr_type) {
      const rank = HDR_RANK[f.hdr_type] ?? 0;
      if (rank > hdrRank) {
        hdrRank = rank;
        hdr = f.hdr_type;
      }
    }
    if (f.width != null && f.width > maxWidth) maxWidth = f.width;
    if (f.height != null && f.height > maxHeight) maxHeight = f.height;
    for (const a of f.audio_streams ?? []) {
      if (a.immersive && a.immersive !== 'none') immersive.add(a.immersive);
    }
    if ((f.subtitle_streams?.length ?? 0) > 0) hasCaptions = true;
  }

  const resLabel =
    maxWidth > 0 || maxHeight > 0
      ? resolutionLabel(maxWidth > 0 ? maxWidth : undefined, maxHeight)
      : undefined;
  const hdrText = hdrLabel(hdr);
  const immersiveLabels = [...immersive].map((k) => IMMERSIVE_LABELS[k]);

  const pills: React.ReactNode[] = [];
  if (resLabel) pills.push(<Pill key="res">{resLabel}</Pill>);
  if (hdrText) pills.push(<Pill key="hdr">{hdrText}</Pill>);
  for (const label of immersiveLabels) pills.push(<Pill key={label}>{label}</Pill>);
  if (hasCaptions) pills.push(<Pill key="cc">CC</Pill>);

  if (pills.length === 0) return null;

  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>{pills}</div>
  );
}
