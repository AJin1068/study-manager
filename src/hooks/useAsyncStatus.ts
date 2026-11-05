/**
 * What a loaded-or-reloading value should display. A failed reload keeps the last good data for
 * layout, but the error is always reported: stale data must never hide a failed read.
 */
export type AsyncStatus<T> =
  | { kind: 'loading' }
  | { kind: 'error'; message: string; stale: T | null }
  | { kind: 'ready'; value: T };

export function asyncStatus<T>(data: T | null, error: string | null): AsyncStatus<T> {
  if (error !== null) return { kind: 'error', message: error, stale: data };
  if (data === null) return { kind: 'loading' };
  return { kind: 'ready', value: data };
}
