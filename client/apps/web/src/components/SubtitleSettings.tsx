/**
 * Subtitle appearance panel (`docs/.tasks/99` C4) — a glass popover of caption-style controls
 * (size / color / background / edge / vertical position). Applies to the native `<track>` path
 * via a scoped `::cue` stylesheet; changes persist in `localStorage`.
 */

import type { SubtitleAppearance } from '../lib/subtitleAppearance';
import { DEFAULT_APPEARANCE } from '../lib/subtitleAppearance';
import { theme } from '../theme';

const glass: React.CSSProperties = {
  background: 'rgba(28,28,32,0.62)',
  backdropFilter: 'blur(22px) saturate(160%)',
  WebkitBackdropFilter: 'blur(22px) saturate(160%)',
  border: '1px solid rgba(255,255,255,0.14)',
};

export function SubtitleSettings({
  value,
  onChange,
  onClose,
}: {
  value: SubtitleAppearance;
  onChange: (next: SubtitleAppearance) => void;
  onClose: () => void;
}) {
  const set = <K extends keyof SubtitleAppearance>(key: K, v: SubtitleAppearance[K]) =>
    onChange({ ...value, [key]: v });

  return (
    <div
      role="dialog"
      aria-label="Subtitle appearance"
      style={{
        ...glass,
        position: 'absolute',
        bottom: 92,
        right: 24,
        zIndex: 30,
        width: 300,
        borderRadius: 16,
        padding: 18,
        color: '#fff',
        boxShadow: '0 16px 48px rgba(0,0,0,0.55)',
        display: 'flex',
        flexDirection: 'column',
        gap: 16,
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <span style={{ fontSize: 14, fontWeight: 600 }}>Subtitle appearance</span>
        <button
          type="button"
          onClick={onClose}
          aria-label="Close"
          style={{ background: 'transparent', border: 'none', color: 'rgba(255,255,255,0.7)', cursor: 'pointer', fontSize: 18, lineHeight: 1 }}
        >
          ×
        </button>
      </div>

      <Row label={`Text size — ${value.fontSizePct}%`}>
        <input
          type="range"
          min={50}
          max={200}
          step={10}
          value={value.fontSizePct}
          onChange={(e) => set('fontSizePct', Number(e.target.value))}
          style={{ width: '100%' }}
        />
      </Row>

      <Row label="Text color">
        <input
          type="color"
          value={value.textColor}
          onChange={(e) => set('textColor', e.target.value)}
          style={{ width: 44, height: 28, background: 'transparent', border: 'none', cursor: 'pointer' }}
        />
      </Row>

      <Row label={`Background — ${value.backgroundOpacity}%`}>
        <input
          type="range"
          min={0}
          max={100}
          step={5}
          value={value.backgroundOpacity}
          onChange={(e) => set('backgroundOpacity', Number(e.target.value))}
          style={{ width: '100%' }}
        />
      </Row>

      <Row label="Edge style">
        <select
          value={value.edgeStyle}
          onChange={(e) => set('edgeStyle', e.target.value as SubtitleAppearance['edgeStyle'])}
          style={{ width: '100%', padding: '6px 8px', borderRadius: 8, background: 'rgba(0,0,0,0.4)', color: '#fff', border: '1px solid rgba(255,255,255,0.18)' }}
        >
          <option value="none">None</option>
          <option value="dropShadow">Drop shadow</option>
          <option value="outline">Outline</option>
        </select>
      </Row>

      <Row label={`Position from bottom — ${value.bottomOffsetVh}`}>
        <input
          type="range"
          min={0}
          max={30}
          step={1}
          value={value.bottomOffsetVh}
          onChange={(e) => set('bottomOffsetVh', Number(e.target.value))}
          style={{ width: '100%' }}
        />
      </Row>

      <button
        type="button"
        onClick={() => onChange({ ...DEFAULT_APPEARANCE })}
        style={{
          marginTop: 2,
          padding: '8px 10px',
          borderRadius: 9,
          border: '1px solid rgba(255,255,255,0.18)',
          background: 'transparent',
          color: theme.colors.accent,
          fontSize: 13,
          cursor: 'pointer',
        }}
      >
        Reset to defaults
      </button>
    </div>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label style={{ display: 'flex', flexDirection: 'column', gap: 6, fontSize: 12, color: 'rgba(255,255,255,0.8)' }}>
      <span>{label}</span>
      {children}
    </label>
  );
}
