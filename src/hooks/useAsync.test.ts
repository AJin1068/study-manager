import { describe, expect, it } from 'vitest';
import { nextAsyncState, type AsyncState } from './useAsync';

const empty: AsyncState<number> = { data: null, error: null };

describe('nextAsyncState (useAsync contract)', () => {
  it('a successful load sets data and clears any error', () => {
    const failed: AsyncState<number> = { data: null, error: 'boom' };
    expect(nextAsyncState(failed, { ok: true, value: 3 })).toEqual({ data: 3, error: null });
  });

  it('a failed first load sets the error and leaves no data', () => {
    expect(nextAsyncState(empty, { ok: false, error: 'boom' })).toEqual({ data: null, error: 'boom' });
  });

  it('a failed reload keeps the last good data and sets the error', () => {
    const loaded: AsyncState<number> = { data: 7, error: null };
    expect(nextAsyncState(loaded, { ok: false, error: 'reload failed' })).toEqual({
      data: 7,
      error: 'reload failed',
    });
  });

  it('a successful reload after a failure recovers and clears the error', () => {
    const stale: AsyncState<number> = { data: 7, error: 'reload failed' };
    expect(nextAsyncState(stale, { ok: true, value: 9 })).toEqual({ data: 9, error: null });
  });

  it('stringifies non-string errors so the UI always gets a message', () => {
    expect(nextAsyncState(empty, { ok: false, error: { code: 1 } }).error).toBe('[object Object]');
  });
});
