/**
 * `Status` primitives (Task 81) — the shared loading / error / empty / not-found blocks
 * every page uses, so those states look identical app-wide and pages stay thin. Restyle
 * an app state once here rather than in each page.
 */

import type { ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { theme } from '../theme';

export function Loading({ label = 'Loading…' }: { label?: string }) {
  return <p style={{ color: theme.colors.textMuted, padding: '24px 0' }}>{label}</p>;
}

export function ErrorState({ message }: { message: string }) {
  return (
    <p style={{ color: '#ff6b6b', padding: '24px 0' }}>Something went wrong: {message}</p>
  );
}

export function EmptyState({ children }: { children: ReactNode }) {
  return <p style={{ color: theme.colors.textMuted, padding: '24px 0' }}>{children}</p>;
}

export function NotFound({
  title = 'Not found',
  message = "We couldn't find that title.",
}: {
  title?: string;
  message?: string;
}) {
  return (
    <section style={{ padding: '24px 0' }}>
      <h1 style={{ fontSize: 24, margin: '0 0 8px', color: theme.colors.text }}>{title}</h1>
      <p style={{ color: theme.colors.textMuted, margin: '0 0 16px' }}>{message}</p>
      <Link to="/" style={{ color: theme.colors.accent }}>
        Back to the library
      </Link>
    </section>
  );
}
