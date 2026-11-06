import { describe, expect, it } from 'vitest';
import type { PassStatus, TimerState } from '../api';
import { availableKinds, endsChallenge, outcomeMessage, passStatusText } from './releaseView';

const locked: TimerState = {
  phase: 'study',
  runId: 1,
  segmentId: 2,
  setIndex: 1,
  setsTotal: 3,
  plannedEndMs: 10_000_000,
  lockHeld: true,
  nowMs: 9_000_000,
  next: null,
};
const idle: TimerState = { ...locked, phase: 'idle', runId: null, segmentId: null, lockHeld: false, plannedEndMs: null };
const pass: PassStatus = { dailyLimit: 1, usedToday: 0, remainingToday: 1, activeUntilMs: null, durationMin: 10, penaltyPoints: 5 };
const upcoming = {
  planId: 1,
  name: 'a',
  subject: 'b',
  date: '2026-11-05',
  sets: 1,
  plannedStartMs: 1,
  plannedEndMs: 2,
};

describe('release view helpers', () => {
  it('offers every release while locked and a pass is left', () => {
    expect(availableKinds(locked, pass)).toEqual(['emergencyPass', 'skipToday', 'earlyRelease']);
  });

  it('hides the pass when the allowance is spent or a pass is already running', () => {
    expect(availableKinds(locked, { ...pass, remainingToday: 0, usedToday: 1 })).toEqual(['skipToday', 'earlyRelease']);
    expect(availableKinds(locked, { ...pass, activeUntilMs: 5 })).toEqual(['skipToday', 'earlyRelease']);
    expect(availableKinds(locked, null)).toEqual(['skipToday', 'earlyRelease']);
  });

  it('offers only a skip before the plan starts, and nothing without a plan or state', () => {
    expect(availableKinds({ ...idle, next: upcoming }, pass)).toEqual(['skipToday']);
    expect(availableKinds(idle, pass)).toEqual([]);
    expect(availableKinds(null, pass)).toEqual([]);
  });

  it('only a mismatch keeps the challenge open', () => {
    expect(endsChallenge({ outcome: 'mismatch', attempts: 1 })).toBe(false);
    expect(endsChallenge({ outcome: 'noChallenge' })).toBe(true);
    expect(endsChallenge({ outcome: 'limitReached', limit: 1 })).toBe(true);
    expect(endsChallenge({ outcome: 'released', kind: 'skipToday', expiresAtMs: null })).toBe(true);
  });

  it('words every outcome and states the cost of a pass', () => {
    expect(outcomeMessage({ outcome: 'mismatch', attempts: 2 })).toContain('2번째');
    expect(outcomeMessage({ outcome: 'limitReached', limit: 2 })).toContain('2회');
    expect(outcomeMessage({ outcome: 'released', kind: 'emergencyPass', expiresAtMs: 1 })).toContain('포인트 차감');
    expect(outcomeMessage({ outcome: 'released', kind: 'skipToday', expiresAtMs: null })).toContain('건너뛰었습니다');
    expect(outcomeMessage({ outcome: 'released', kind: 'earlyRelease', expiresAtMs: null })).toContain('조기 해제');
    expect(outcomeMessage({ outcome: 'nothingToRelease' })).toContain('없습니다');
    expect(outcomeMessage({ outcome: 'alreadyActive', expiresAtMs: 1 })).toContain('적용 중');
    expect(outcomeMessage({ outcome: 'noChallenge' })).toContain('다시');
  });

  it('shows the remaining allowance, or the running pass', () => {
    expect(passStatusText(pass, 0)).toBe('긴급 패스 오늘 1/1회 남음 · 10분 · -5점');
    expect(passStatusText({ ...pass, activeUntilMs: 100 }, 50)).toContain('적용 중');
    expect(passStatusText({ ...pass, activeUntilMs: 100 }, 100)).toContain('남음');
  });
});
