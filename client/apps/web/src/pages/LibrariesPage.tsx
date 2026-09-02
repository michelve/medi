/**
 * `LibrariesPage` (Task 82) — `/settings/libraries`, the Plex-style admin panel.
 *
 * Lists libraries (`client.libraries()`) and hosts a `LibraryEditor` per row for the
 * rename / add-remove-folder / rescan / delete writes, plus a create form
 * (`client.createLibrary`). Every successful write refreshes the list. Creating a library
 * and rescanning it repopulates the browse grid (`81`) with its titles.
 */

import { useCallback, useEffect, useState } from 'react';
import { ApiError, type Library, type LibraryTypeKind } from '@medi/api-client';
import { useApi } from '../api';
import { LibraryEditor } from '../components/LibraryEditor';
import { Loading, ErrorState } from '../components/Status';
import { theme } from '../theme';

export function LibrariesPage() {
  const api = useApi();
  const [libraries, setLibraries] = useState<Library[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Create-form state.
  const [name, setName] = useState('');
  const [kind, setKind] = useState<LibraryTypeKind>('movie');
  const [folder, setFolder] = useState('');
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    const controller = new AbortController();
    api
      .libraries({ signal: controller.signal })
      .then((libs) => {
        if (!controller.signal.aborted) {
          setLibraries(libs);
          setError(null);
        }
      })
      .catch((err: unknown) => {
        if (controller.signal.aborted) return;
        setError(err instanceof ApiError ? err.message : String(err));
      });
    return () => controller.abort();
  }, [api]);

  useEffect(() => refresh(), [refresh]);

  const create = async () => {
    const trimmedName = name.trim();
    const trimmedFolder = folder.trim();
    if (!trimmedName || !trimmedFolder || creating) return;
    setCreating(true);
    setCreateError(null);
    try {
      await api.createLibrary({ name: trimmedName, kind, folders: [trimmedFolder] });
      setName('');
      setFolder('');
      refresh();
    } catch (err) {
      setCreateError(err instanceof ApiError ? err.message : String(err));
    } finally {
      setCreating(false);
    }
  };

  if (libraries === null && error === null) return <Loading label="Loading libraries…" />;
  if (error && libraries === null) return <ErrorState message={error} />;

  return (
    <section style={{ maxWidth: 760 }}>
      <h1 style={{ fontSize: 24, margin: '0 0 4px' }}>Libraries</h1>
      <p style={{ color: theme.colors.textMuted, margin: '0 0 24px', fontSize: 14 }}>
        Add a folder under your media root, then rescan to populate the grid.
      </p>

      {/* Create form */}
      <div
        style={{
          padding: 16,
          borderRadius: 10,
          background: theme.colors.surface,
          display: 'grid',
          gap: 10,
          marginBottom: 28,
        }}
      >
        <div style={{ fontSize: 14, fontWeight: 600 }}>New library</div>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <input
            className="medi-search-input"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Name (e.g. Movies)"
            aria-label="New library name"
            style={{ ...inputStyle, flex: '1 1 160px' }}
          />
          <select
            value={kind}
            onChange={(e) => setKind(e.target.value as LibraryTypeKind)}
            aria-label="Library type"
            style={inputStyle}
          >
            <option value="movie">Movies</option>
            <option value="series">Series</option>
          </select>
        </div>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <input
            className="medi-search-input"
            value={folder}
            onChange={(e) => setFolder(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && create()}
            placeholder="/media/movies"
            aria-label="First folder path"
            style={{ ...inputStyle, flex: '1 1 240px' }}
          />
          <button type="button" onClick={create} disabled={creating} style={primaryBtn}>
            Create
          </button>
        </div>
        {createError && <p style={{ margin: 0, fontSize: 13, color: '#ff6b6b' }}>{createError}</p>}
      </div>

      {/* Existing libraries */}
      <div style={{ display: 'grid', gap: 16 }}>
        {libraries && libraries.length === 0 && (
          <p style={{ color: theme.colors.textMuted }}>No libraries yet — create one above.</p>
        )}
        {libraries?.map((lib) => (
          <LibraryEditor key={lib.id} library={lib} onChanged={refresh} />
        ))}
      </div>
    </section>
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
  padding: '8px 16px',
  borderRadius: 8,
  border: 'none',
  fontSize: 14,
  fontWeight: 600,
  cursor: 'pointer',
  color: '#fff',
  background: theme.colors.accent,
};
