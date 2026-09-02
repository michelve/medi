/**
 * `useDebouncedValue` (Task 81) — returns `value` after it has stayed unchanged for
 * `delayMs`. Used to debounce the search box so each keystroke doesn't re-filter the grid.
 */

import { useEffect, useState } from 'react';

export function useDebouncedValue<T>(value: T, delayMs = 200): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(timer);
  }, [value, delayMs]);
  return debounced;
}
