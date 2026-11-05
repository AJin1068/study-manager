import { useCallback, useEffect, useState } from 'react';

export interface AsyncState<T> {
  data: T | null;
  error: string | null;
}

export type AsyncResult<T> = { ok: true; value: T } | { ok: false; error: unknown };

/**
 * Pure transition for a finished load. The contract, tested directly:
 * - success replaces `data` and clears `error`;
 * - failure keeps the last good `data` (so the page does not blank out) and always sets `error`.
 * Callers must render `error` regardless of `data`; stale data never hides a failed read.
 */
export function nextAsyncState<T>(prev: AsyncState<T>, result: AsyncResult<T>): AsyncState<T> {
  if (result.ok) return { data: result.value, error: null };
  return { data: prev.data, error: String(result.error) };
}

/**
 * Loads data through a command wrapper. Pass a stable function reference (for example
 * `commands.getModeConfig`), not an inline arrow, so the loader is not re-run every render.
 */
export function useAsync<T>(load: () => Promise<T>) {
  const [state, setState] = useState<AsyncState<T>>({ data: null, error: null });

  const reload = useCallback(() => {
    load()
      .then((value) => setState((prev) => nextAsyncState(prev, { ok: true, value })))
      .catch((error: unknown) => setState((prev) => nextAsyncState(prev, { ok: false, error })));
  }, [load]);

  useEffect(() => {
    reload();
  }, [reload]);

  return { data: state.data, error: state.error, reload };
}
