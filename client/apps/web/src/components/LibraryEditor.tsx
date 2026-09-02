/**
 * `LibraryEditor` (Task 82) — edit one library: rename, add/remove folders, rescan, delete.
 *
 * Each write goes through the api-client (`patchLibrary`, `scanLibrary`, `deleteLibrary`)
 * and reports failures via the shared `ApiError` mapping: a `409` from a scan
 * (`isBusy`) shows "scan already in progress"; a `404` a not-found note; anything else the
 * server's `error.message`. On success it asks the page to refresh the list (`onChanged`)
 * so the grid/state stay in sync.
 */

import { useState } from 'react';
import { ApiError, type Library } from '@medi/api-client';
import { useApi } from '../api';
import { theme } from '../theme';

export interface LibraryEditorProps {
  library: Library;
  /** Re-fetch the library list after any successful write. */
  onChanged: () => void;
}

/** Turn any thrown error into a user-facing string using the ApiError contract. */
function messageFor(err: unknown): string {
  if (err instanceof ApiError) {
    if (err.isBusy) return 'A scan is already in progress for this library.';
    if (err.isNotFound) return 'This library no longer exists — refresh the list.';
    return err.message;
  }
  return String(err);
}

export function LibraryEditor({ library, onChanged }: LibraryEditorProps) {
  const api = useApi();
  const [name, setName] = useState(library.name);
  const [newFolder, setNewFolder] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  // Run a write, surface its error, and refresh on success. Guards double-submits.
  const run = async (op: () => Promise<unknown>, successNotice?: string) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await op();
      if (successNotice) setNotice(successNotice);
      onChanged();
    } catch (err) {
      setError(messageFor(err));
    } finally {
      setBusy(false);
    }
  };

  const rename = () => {
    const trimmed = name.trim();
    if (!trimmed || trimmed === library.name) return;
    void run(() => api.patchLibrary(library.id, { name: trimmed }));
  };
  const addFolder = () => {
    const folder = newFolder.trim();
    if (!folder) return;
    void run(async () => {
      await api.patchLibrary(library.id, { add_folders: [folder] });
      setNewFolder('');
    });
  };
  const removeFolder = (folder: string) =>
    run(() => api.patchLibrary(library.id, { remove_folders: [folder] }));
  const scan = () => run(() => api.scanLibrary(library.id), 'Scan started.');
  const remove = () => {
    if (!window.confirm(`Delete library “${library.name}”? Its titles are removed from the catalog.`)) {
      return;
    }
    void run(() => api.deleteLibrary(library.id));
  };

  return (
    <div
      style={{
        padding: 16,
        borderRadius: 10,
        background: theme.colors.surface,
        display: 'grid',
        gap: 12,
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
        <input
          className="medi-search-input"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={rename}
          onKeyDown={(e) => e.key === 'Enter' && rename()}
          aria-label="Library name"
          style={inputStyle}
        />
        <span
          style={{
            fontSize: 12,
            color: theme.colors.textMuted,
            border: `1px solid ${theme.colors.textMuted}`,
            borderRadius: 4,
            padding: '2px 6px',
            textTransform: 'uppercase',
          }}
        >
          {library.kind}
        </span>
        <div style={{ flex: 1 }} />
        <button type="button" onClick={scan} disabled={busy} style={primaryBtn}>
          Rescan
        </button>
        <button type="button" onClick={remove} disabled={busy} style={dangerBtn}>
          Delete
        </button>
      </div>

      <div>
        <div style={{ fontSize: 13, color: theme.colors.textMuted, marginBottom: 6 }}>
          Folders (must live under MEDIA_DIR)
        </div>
        <ul style={{ listStyle: 'none', padding: 0, margin: 0, display: 'grid', gap: 6 }}>
          {library.folders.map((folder) => (
            <li
              key={folder}
              style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 13 }}
            >
              <code style={{ color: theme.colors.text, wordBreak: 'break-all' }}>{folder}</code>
              <button
                type="button"
                onClick={() => removeFolder(folder)}
                disabled={busy}
                aria-label={`Remove ${folder}`}
                style={linkBtn}
              >
                remove
              </button>
            </li>
          ))}
          {library.folders.length === 0 && (
            <li style={{ fontSize: 13, color: theme.colors.textMuted }}>No folders yet.</li>
          )}
        </ul>
        <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
          <input
            className="medi-search-input"
            value={newFolder}
            onChange={(e) => setNewFolder(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && addFolder()}
            placeholder="/media/movies/…"
            aria-label="Add folder path"
            style={{ ...inputStyle, flex: 1 }}
          />
          <button type="button" onClick={addFolder} disabled={busy} style={primaryBtn}>
            Add
          </button>
        </div>
      </div>

      {notice && <p style={{ margin: 0, fontSize: 13, color: theme.colors.accent }}>{notice}</p>}
      {error && <p style={{ margin: 0, fontSize: 13, color: '#ff6b6b' }}>{error}</p>}
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
};
const dangerBtn: React.CSSProperties = {
  ...primaryBtn,
  background: 'transparent',
  color: '#ff6b6b',
  border: '1px solid #ff6b6b',
};
const linkBtn: React.CSSProperties = {
  border: 'none',
  background: 'transparent',
  color: theme.colors.textMuted,
  cursor: 'pointer',
  fontSize: 12,
  textDecoration: 'underline',
};
