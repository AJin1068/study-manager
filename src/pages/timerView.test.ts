import { describe, expect, it } from 'vitest';
import type { TimerState } from '../api';
import {
  attemptText,
  formatRemaining,
  formatStudied,
  newerState,
  phaseText,
  remainingMs,
  runSummaryText,
  setSummaryText,
} from './timerView';

const base: TimerState = {
  phase: 'break',
  runId: 1,
  segmentId: 2,
  setIndex: 2,
  setsTotal: 3,
  plannedEndMs: 10_000_000,
  lockHeld: true,
  nowMs: 9_000_000,
  next: null,
};

describe('timer view helpers', () => {
  it('keeps the fresher state when events arrive out of order', () => {
    const older = { ...base, nowMs: 1 };
    const newer = { ...base, nowMs: 2 };
    expect(newerState(newer, older)).toBe(newer);
    expect(newerState(older, newer)).toBe(newer);
    expect(newerState(null, older)).toBe(older);
  });

  it('counts down against the core clock, not the local one', () => {
    // Local clock is 5 s behind the core clock (offset +5000).
    expect(remainingMs(base, 5_000, 9_000_000 - 5_000 + 60_000)).toBe(940_000);
  });

  it('clamps at zero and has no countdown while idle', () => {
    expect(remainingMs(base, 0, 99_000_000)).toBe(0);
    expect(remainingMs({ ...base, plannedEndMs: null }, 0, 0)).toBeNull();
  });

  it('formats mm:ss and h:mm:ss, rounding partial seconds up', () => {
    expect(formatRemaining(61_000)).toBe('01:01');
    expect(formatRemaining(500)).toBe('00:01');
    expect(formatRemaining(3_725_000)).toBe('1:02:05');
  });

  it('describes the phase and set, and a held lock with no block', () => {
    expect(phaseText(base)).toBe('휴식 중 · 2/3세트');
    expect(phaseText({ ...base, phase: 'idle', lockHeld: false })).toBe('공부 외 시간');
    expect(phaseText({ ...base, phase: 'idle', lockHeld: true })).toBe('계획 진행 중');
  });
});

describe('summary text', () => {
  it('formats studied time', () => {
    const table: Array<[number, string]> = [
      [0, '1분 미만'],
      [59_999, '1분 미만'],
      [25 * 60_000, '25분'],
      [60 * 60_000, '1시간'],
      [75 * 60_000, '1시간 15분'],
    ];
    for (const [ms, text] of table) expect(formatStudied(ms)).toBe(text);
  });

  it('shows the measured attempt count, including 0', () => {
    expect(attemptText(0)).toBe('딴짓 시도 0회');
    const set = { runId: 1, setIndex: 2, setStudiedMs: 25 * 60_000, todayStudiedMs: 50 * 60_000,
      attemptCount: 3, focusCreditedMs: 0, focusScore: null, firstAwayAfterMs: null, returnAfterMs: null };
    expect(setSummaryText(set)).toBe('2세트 끝 · 이번 세트 25분 · 오늘 50분 · 딴짓 시도 3회');
    const run = { runId: 1, setsTotal: 3, runStudiedMs: 75 * 60_000, todayStudiedMs: 75 * 60_000, attemptCount: 0, focusCreditedMs: 0, focusScore: null, status: 'aborted' };
    expect(runSummaryText(run)).toContain('계획 중단');
  });
});
