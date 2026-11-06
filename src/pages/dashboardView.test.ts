import { describe, expect, it } from 'vitest';
import {
  appOffText,
  barPercent,
  durationText,
  firstAwayText,
  heatLevel,
  heatmapIsEmpty,
  heatmapMax,
  nextPlanText,
  pctText,
  phonePickupText,
  phoneUnreachableText,
  startsInText,
  streakText,
} from './dashboardView';

const MIN = 60_000;

describe('dashboard text', () => {
  it('formats durations without rounding up', () => {
    expect(durationText(0)).toBe('0분');
    expect(durationText(59_999)).toBe('0분');
    expect(durationText(25 * MIN)).toBe('25분');
    expect(durationText(60 * MIN)).toBe('1시간');
    expect(durationText(75 * MIN)).toBe('1시간 15분');
    expect(durationText(-5)).toBe('0분');
  });

  it('shows no plan as a state, not as 0%', () => {
    expect(pctText(null)).toBe('계획 없음');
    expect(pctText(0)).toBe('0%');
    expect(pctText(80)).toBe('80%');
  });

  it('counts down to the next plan and handles none', () => {
    expect(startsInText(30_000)).toBe('곧');
    expect(startsInText(125 * MIN)).toBe('2시간 5분 후');
    expect(nextPlanText({ nextPlan: null })).toBe('예정된 계획이 없습니다');
    const plan = { planId: 1, name: '수학', subject: '수학', plannedStartMs: 0, startsInMs: 25 * MIN };
    expect(nextPlanText({ nextPlan: plan })).toBe('수학 (수학) · 25분 후');
  });

  it('explains the streak rule when it is zero', () => {
    expect(streakText({ streakDays: 0, streakThresholdPct: 80 })).toContain('80%');
    expect(streakText({ streakDays: 3, streakThresholdPct: 80 })).toBe('3일 연속 달성');
  });

  it('describes first-away and app-off stats, including empty ones', () => {
    expect(firstAwayText(null, 0, 0)).toBe('기록된 공부 세트가 없습니다');
    expect(firstAwayText(null, 0, 4)).toBe('딴짓 없이 끝난 세트 4개');
    expect(firstAwayText(20 * MIN, 2, 4)).toBe('평균 20분 만에 첫 이탈 (이탈한 세트 2/4)');
    expect(appOffText({ sleepMs: 0, forcedQuitMs: 0, forcedQuitCount: 0, plannedOverlapMs: 0 })).toBe('꺼져 있던 시간이 없습니다');
    const off = appOffText({ sleepMs: 30 * MIN, forcedQuitMs: 20 * MIN, forcedQuitCount: 1, plannedOverlapMs: 30 * MIN });
    expect(off).toBe('절전 30분 · 앱 꺼짐 20분 (1회) · 계획 시간과 겹친 시간 30분');
  });
});

describe('chart levels', () => {
  it('caps bars at the full width and survives an empty whole', () => {
    expect(barPercent(50, 100)).toBe(50);
    expect(barPercent(150, 100)).toBe(100);
    expect(barPercent(5, 0)).toBe(0);
  });

  it('heat level is 0 only for an empty cell and 4 for the busiest', () => {
    expect(heatLevel(0, 10)).toBe(0);
    expect(heatLevel(1, 10)).toBe(1);
    expect(heatLevel(10, 10)).toBe(4);
    expect(heatLevel(3, 0)).toBe(0);
  });

  it('detects an empty heatmap', () => {
    const empty = Array.from({ length: 7 }, () => Array.from({ length: 24 }, () => 0));
    expect(heatmapIsEmpty(empty)).toBe(true);
    const row = empty[3];
    if (row === undefined) throw new Error('row');
    row[20] = 2;
    expect(heatmapMax(empty)).toBe(2);
    expect(heatmapIsEmpty(empty)).toBe(false);
  });
});

describe('phone stats', () => {
  const none = { unreachableMs: 0, gaps: 0, serviceDisabledGaps: 0, pickupSetsReported: 0, pickupSetsWithPickups: 0, pickupsTotal: 0 };

  it('says so when the phone never left the lock or reported nothing', () => {
    expect(phoneUnreachableText(none)).toBe('폰이 잠금에서 벗어난 시간이 없습니다');
    expect(phonePickupText(none)).toBe('폰이 보고한 공부 세트가 없습니다');
  });

  it('shows the unreachable time on its own with the number of breaks', () => {
    expect(phoneUnreachableText({ ...none, unreachableMs: 50 * MIN, gaps: 3 })).toBe('폰 미연결 시간 50분 · 3번 끊김');
    expect(phoneUnreachableText({ ...none, unreachableMs: 10 * MIN, gaps: 2, serviceDisabledGaps: 1 })).toContain('차단 서비스를 끈 적 1회');
  });

  it('counts pick-ups without judging them', () => {
    const text = phonePickupText({ ...none, pickupSetsReported: 3, pickupSetsWithPickups: 2, pickupsTotal: 7 });
    expect(text).toBe('공부 중 폰을 집어 든 횟수 7회 · 집어 든 세트 2/3');
    expect(text).not.toMatch(/점수|감점|나쁨/);
  });
});
