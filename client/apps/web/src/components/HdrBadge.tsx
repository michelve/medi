/**
 * `HdrBadge` (Task 81) — a small pill mapping a title's HDR tier to a human label.
 *
 * The backend emits loose ffprobe-fidelity strings (`HdrTier`); we map the known
 * values to their marketing labels and fall back to an upper-cased raw string for
 * anything new the prober learns to report. Renders nothing when there is no HDR.
 */

import type { HdrTier } from '@medi/api-client';
import { theme } from '../theme';

/** Marketing labels for the known HDR tiers; unknowns pass through upper-cased. */
const LABELS: Record<string, string> = {
  dolbyvision: 'DV',
  hdr10: 'HDR10',
  hdr10plus: 'HDR10+',
  hlg: 'HLG',
};

export function hdrLabel(hdr: HdrTier | null | undefined): string | undefined {
  if (!hdr) return undefined;
  return LABELS[hdr] ?? hdr.toUpperCase();
}

export function HdrBadge({ hdr }: { hdr: HdrTier | null | undefined }) {
  const label = hdrLabel(hdr);
  if (!label) return null;
  return (
    <span
      style={{
        display: 'inline-block',
        padding: '2px 6px',
        borderRadius: 4,
        fontSize: 11,
        fontWeight: 700,
        letterSpacing: 0.4,
        lineHeight: 1.2,
        color: theme.colors.text,
        background: 'rgba(255, 255, 255, 0.16)',
        border: `1px solid ${theme.colors.textMuted}`,
        whiteSpace: 'nowrap',
      }}
    >
      {label}
    </span>
  );
}
