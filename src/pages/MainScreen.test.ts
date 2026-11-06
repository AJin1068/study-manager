import { describe, expect, it } from 'vitest';
import { lockStatusText } from './MainScreen';

describe('lockStatusText precedence', () => {
  it('shows the load error, not loading or a state, when the read failed', () => {
    expect(lockStatusText({ loaded: false, lockHeld: null, error: 'db down' })).toBe(
      '상태를 읽지 못했습니다: db down',
    );
  });

  it('shows the error even when a previous state is still held', () => {
    expect(lockStatusText({ loaded: true, lockHeld: false, error: 'stale' })).toBe(
      '상태를 읽지 못했습니다: stale',
    );
  });

  it('shows loading before the first load completes', () => {
    expect(lockStatusText({ loaded: false, lockHeld: null, error: null })).toBe('불러오는 중');
  });

  it('shows the lock-held state', () => {
    expect(lockStatusText({ loaded: true, lockHeld: true, error: null })).toBe('잠금 중(계획 진행)');
  });

  it('shows lock-free only for a known false', () => {
    expect(lockStatusText({ loaded: true, lockHeld: false, error: null })).toBe('공부 외 시간');
  });
});
