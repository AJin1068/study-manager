// Display helpers for the dashboard. The Rust core computes every number; these only turn them
// into short Korean text and chart levels. Empty data is a normal state, never an error.

import type { AppOffStat, ReleaseUsage, TodayDashboard } from '../api';

export const WEEKDAY_LABELS = ['월', '화', '수', '목', '금', '토', '일'] as const;

/** `1시간 15분`, `25분`, `0분`. */
export function durationText(ms: number): string {
  const minutes = Math.floor(Math.max(0, ms) / 60_000);
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (hours === 0) return `${rest}분`;
  return rest === 0 ? `${hours}시간` : `${hours}시간 ${rest}분`;
}

export function pctText(pct: number | null): string {
  return pct === null ? '계획 없음' : `${pct}%`;
}

export function scoreText(score: number | null): string {
  return score === null ? '-' : `${score}점`;
}

/** `곧`, `25분 후`, `2시간 5분 후`. */
export function startsInText(ms: number): string {
  if (ms < 60_000) return '곧';
  return `${durationText(ms)} 후`;
}

export function nextPlanText(today: Pick<TodayDashboard, 'nextPlan'>): string {
  const next = today.nextPlan;
  if (next === null) return '예정된 계획이 없습니다';
  return `${next.name} (${next.subject}) · ${startsInText(next.startsInMs)}`;
}

export function streakText(today: Pick<TodayDashboard, 'streakDays' | 'streakThresholdPct'>): string {
  if (today.streakDays === 0) return `연속 달성 0일 (하루 계획의 ${today.streakThresholdPct}% 이상이면 달성)`;
  return `${today.streakDays}일 연속 달성`;
}

/** Width of a bar as a whole percent of `whole`, never above 100. */
export function barPercent(part: number, whole: number): number {
  if (whole <= 0) return 0;
  return Math.min(100, Math.round((part * 100) / whole));
}

/** Heat level 0 to 4: 0 only for an empty cell, otherwise scaled to the busiest cell. */
export function heatLevel(count: number, max: number): number {
  if (count <= 0 || max <= 0) return 0;
  return Math.min(4, Math.max(1, Math.ceil((count * 4) / max)));
}

export function heatmapMax(heatmap: number[][]): number {
  return Math.max(0, ...heatmap.map((row) => Math.max(0, ...row)));
}

export function heatmapIsEmpty(heatmap: number[][]): boolean {
  return heatmapMax(heatmap) === 0;
}

export function firstAwayText(avgMs: number | null, setsWithAway: number, setsTotal: number): string {
  if (setsTotal === 0) return '기록된 공부 세트가 없습니다';
  if (avgMs === null) return `딴짓 없이 끝난 세트 ${setsTotal}개`;
  return `평균 ${durationText(avgMs)} 만에 첫 이탈 (이탈한 세트 ${setsWithAway}/${setsTotal})`;
}

/** The "app was off" line: separate from effort, and says how much of it hit a plan. */
export function appOffText(off: AppOffStat): string {
  if (off.sleepMs === 0 && off.forcedQuitMs === 0) return '꺼져 있던 시간이 없습니다';
  const parts = [`절전 ${durationText(off.sleepMs)}`, `앱 꺼짐 ${durationText(off.forcedQuitMs)} (${off.forcedQuitCount}회)`];
  return `${parts.join(' · ')} · 계획 시간과 겹친 시간 ${durationText(off.plannedOverlapMs)}`;
}

export function releaseText(usage: ReleaseUsage): string {
  return `긴급 패스 ${usage.emergencyPass}회 · 오늘 건너뛰기 ${usage.skipToday}회 · 중간 해제 ${usage.earlyRelease}회 · 중단된 계획 ${usage.abortedRuns}회`;
}
