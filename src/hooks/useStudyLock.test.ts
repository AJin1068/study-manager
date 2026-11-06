import { describe, expect, it } from 'vitest';
import { describeOutcome, lockView } from './useStudyLock';

const free = { lockHeld: false, changedAtMs: 0 };
const held = { lockHeld: true, changedAtMs: 1 };

describe('lockView (fail-closed mapping from load result to UI gate)', () => {
  it('allows changes only when loaded, lock free, and without error', () => {
    expect(lockView(free, null)).toEqual({ loaded: true, lockHeld: false, error: null, allowed: true });
  });

  it('blocks changes while the lock is held and reports lockHeld=true', () => {
    const view = lockView(held, null);
    expect(view.lockHeld).toBe(true);
    expect(view.allowed).toBe(false);
  });

  it('before the first load, lockHeld is unknown (null), not false', () => {
    const view = lockView(null, null);
    expect(view.loaded).toBe(false);
    expect(view.lockHeld).toBeNull();
    expect(view.allowed).toBe(false);
  });

  it('a failed initial load reports lockHeld as unknown and blocks changes', () => {
    const view = lockView(null, 'db unavailable');
    expect(view.lockHeld).toBeNull();
    expect(view.error).toBe('db unavailable');
    expect(view.allowed).toBe(false);
  });

  it('a populated state with a load error still blocks changes', () => {
    expect(lockView(free, 'partial failure').allowed).toBe(false);
  });
});

describe('describeOutcome', () => {
  it('reports applied changes', () => {
    expect(describeOutcome({ outcome: 'applied', key: 'autostart' })).toBe('변경했습니다.');
  });

  it('reports rejected attempts as recorded, not silently dropped', () => {
    expect(describeOutcome({ outcome: 'rejectedLocked', key: 'autostart' })).toContain('기록에 남았습니다');
  });
});
