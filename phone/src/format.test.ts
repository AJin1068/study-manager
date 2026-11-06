import { describe, expect, it } from 'vitest';
import { isAck, isPhoneView, type BlockerStatus, type PhoneView } from './api';
import { blockerText, errorText, formatDuration, lockIsHollow, lockText } from './format';

const report = (blocker: BlockerStatus, lock: 'locked' | 'free') => ({
  diagnostics: {
    lock: lock === 'locked' ? ({ state: 'locked' } as const) : ({ state: 'free', reason: 'lease_expired' } as const),
    seq: 3,
    lock_remaining_ms: lock === 'locked' ? 60_000 : null,
    timer: lock === 'locked' ? ({ state: 'studying', remaining_ms: 1000 } as const) : ({ state: 'free' } as const),
    last_server_contact_ago_ms: 5000,
    rejected_leases: 0,
  },
  blocker,
  service_enabled: blocker.kind !== 'service_disabled',
  online: true,
  last_error: null,
});

const view = (r: PhoneView['report']): PhoneView => ({ paired: true, pairingLost: false, report: r, platform: 'android', lastError: null });

describe('formatDuration', () => {
  it('formats minutes, hours and clamps negatives', () => {
    expect(formatDuration(0)).toBe('00:00');
    expect(formatDuration(59_999)).toBe('00:59');
    expect(formatDuration(25 * 60_000)).toBe('25:00');
    expect(formatDuration(3_600_000 + 61_000)).toBe('1:01:01');
    expect(formatDuration(-5)).toBe('00:00');
  });
});

describe('texts', () => {
  it('names the reason a phone is free and every blocker state', () => {
    expect(lockText({ state: 'locked' })).toBe('잠금 중');
    expect(lockText({ state: 'free', reason: 'emergency_pass' })).toContain('긴급 패스');
    expect(blockerText({ kind: 'service_disabled' })).toContain('차단되지 않음');
    expect(blockerText({ kind: 'unsupported', reason: 'x' })).toContain('아이폰');
  });

  it('never shows a bare code for a known refusal and still shows unknown ones', () => {
    expect(errorText('used')).toContain('이미 쓴');
    expect(errorText('locked')).toContain('잠금 중');
    expect(errorText('weird')).toBe('실패: weird');
  });
});

describe('lockIsHollow', () => {
  it('flags a lease nobody enforces', () => {
    expect(lockIsHollow(view(report({ kind: 'service_disabled' }, 'locked')))).toBe(true);
    expect(lockIsHollow(view(report({ kind: 'inactive' }, 'locked')))).toBe(true);
    expect(lockIsHollow(view(report({ kind: 'unsupported', reason: 'r' }, 'locked')))).toBe(true);
  });

  it('does not flag an enforced, free or unknown state', () => {
    expect(lockIsHollow(view(report({ kind: 'active' }, 'locked')))).toBe(false);
    expect(lockIsHollow(view(report({ kind: 'service_disabled' }, 'free')))).toBe(false);
    expect(lockIsHollow(view(null))).toBe(false);
  });
});

describe('reply guards', () => {
  it('accepts the shell output and refuses a malformed view', () => {
    expect(isPhoneView(view(report({ kind: 'active' }, 'locked')))).toBe(true);
    expect(isPhoneView(view(null))).toBe(true);
    expect(isPhoneView({ ...view(null), platform: 'windows' })).toBe(false);
    expect(isPhoneView({ ...view(report({ kind: 'active' }, 'locked')), report: { diagnostics: {} } })).toBe(false);
    expect(isPhoneView(null)).toBe(false);
    expect(isAck({ code: 'ok' })).toBe(true);
    expect(isAck({ code: 1 })).toBe(false);
  });
});
