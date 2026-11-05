import { describe, expect, it } from 'vitest';
import { asyncStatus } from './useAsyncStatus';

describe('asyncStatus', () => {
  it('is loading before any data or error', () => {
    expect(asyncStatus<number>(null, null)).toEqual({ kind: 'loading' });
  });

  it('is ready with data and no error', () => {
    expect(asyncStatus(7, null)).toEqual({ kind: 'ready', value: 7 });
  });

  it('is an error when there is no data', () => {
    expect(asyncStatus<number>(null, 'boom')).toEqual({ kind: 'error', message: 'boom', stale: null });
  });

  it('reports the error even when stale data is still held', () => {
    // A failed reload must not look fine just because older data exists.
    expect(asyncStatus(7, 'reload failed')).toEqual({ kind: 'error', message: 'reload failed', stale: 7 });
  });
});
