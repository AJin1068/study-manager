// Mirror of src-tauri/src/commands/dashboard.rs. Types must match the Rust serde output.

import { invoke } from '@tauri-apps/api/core';
import { isPointsStatus, type PointsStatus } from './points';

/** Rust: `dashboard::NextPlan` (named apart from the timer's `NextPlan`). */
export interface DashboardNextPlan {
  planId: number;
  name: string;
  subject: string;
  plannedStartMs: number;
  startsInMs: number;
}

export interface TodayDashboard {
  /** Local date `YYYY-MM-DD`. */
  date: string;
  /** The earliest upcoming plan that has no run yet; null when none within two weeks. */
  nextPlan: DashboardNextPlan | null;
  plannedMs: number;
  /** Recognized study time. */
  actualMs: number;
  /** 0 to 100; null when nothing is planned today. */
  achievementPct: number | null;
  streakDays: number;
  /** Share of a day's plan that counts the day toward the streak. */
  streakThresholdPct: number;
  points: PointsStatus;
}

export interface DayStat {
  date: string;
  /** Monday is 0. */
  weekday: number;
  plannedMs: number;
  actualMs: number;
  achievementPct: number | null;
  achieved: boolean;
  /** Recognized over studied time; null when no study set of the day has finished. */
  focusScore: number | null;
}

export interface SubjectTime {
  subject: string;
  actualMs: number;
}

export interface WeekDashboard {
  weekStart: string;
  weekEnd: string;
  /** Always seven days, Monday first. */
  days: DayStat[];
  plannedMs: number;
  actualMs: number;
  achievementPct: number | null;
  achievedDays: number;
  plannedDays: number;
  bySubject: SubjectTime[];
}

export interface AppCount {
  appId: string;
  count: number;
}

export interface PostponedPlan {
  planId: number;
  name: string;
  subject: string;
  count: number;
}

/** Time the app was off or the machine asleep; shown on its own, never as lack of effort. */
export interface AppOffStat {
  sleepMs: number;
  forcedQuitMs: number;
  forcedQuitCount: number;
  /** Off time that fell inside planned run windows. */
  plannedOverlapMs: number;
}

export interface ReleaseUsage {
  emergencyPass: number;
  skipToday: number;
  earlyRelease: number;
  abortedRuns: number;
}

export interface Weakness {
  fromMs: number;
  toMs: number;
  deviationApps: AppCount[];
  /** Mean time from a set's start to its first deviation, over sets that had one. */
  avgFirstAwayMs: number | null;
  setsWithAway: number;
  setsTotal: number;
  /** `heatmap[weekday][hour]`: seven rows (Monday first) of 24 local hours. */
  heatmap: number[][];
  postponedPlans: PostponedPlan[];
  appOff: AppOffStat;
  releases: ReleaseUsage;
}

type Rec = Record<string, unknown>;

function rec(value: unknown): Rec | null {
  return typeof value === 'object' && value !== null ? (value as Rec) : null;
}

const num = (v: unknown): v is number => typeof v === 'number';
const str = (v: unknown): v is string => typeof v === 'string';
const numOrNull = (v: unknown): v is number | null => v === null || typeof v === 'number';
const listOf = (v: unknown, guard: (item: unknown) => boolean): boolean => Array.isArray(v) && v.every(guard);

export function isDashboardNextPlan(value: unknown): value is DashboardNextPlan {
  const r = rec(value);
  return r !== null && num(r.planId) && str(r.name) && str(r.subject) && num(r.plannedStartMs) && num(r.startsInMs);
}

export function isTodayDashboard(value: unknown): value is TodayDashboard {
  const r = rec(value);
  return (
    r !== null &&
    str(r.date) &&
    (r.nextPlan === null || isDashboardNextPlan(r.nextPlan)) &&
    num(r.plannedMs) &&
    num(r.actualMs) &&
    numOrNull(r.achievementPct) &&
    num(r.streakDays) &&
    num(r.streakThresholdPct) &&
    isPointsStatus(r.points)
  );
}

export function isDayStat(value: unknown): value is DayStat {
  const r = rec(value);
  return (
    r !== null &&
    str(r.date) &&
    num(r.weekday) &&
    num(r.plannedMs) &&
    num(r.actualMs) &&
    numOrNull(r.achievementPct) &&
    typeof r.achieved === 'boolean' &&
    numOrNull(r.focusScore)
  );
}

function isSubjectTime(value: unknown): boolean {
  const r = rec(value);
  return r !== null && str(r.subject) && num(r.actualMs);
}

export function isWeekDashboard(value: unknown): value is WeekDashboard {
  const r = rec(value);
  return (
    r !== null &&
    str(r.weekStart) &&
    str(r.weekEnd) &&
    listOf(r.days, isDayStat) &&
    num(r.plannedMs) &&
    num(r.actualMs) &&
    numOrNull(r.achievementPct) &&
    num(r.achievedDays) &&
    num(r.plannedDays) &&
    listOf(r.bySubject, isSubjectTime)
  );
}

function isAppCount(value: unknown): boolean {
  const r = rec(value);
  return r !== null && str(r.appId) && num(r.count);
}

function isPostponedPlan(value: unknown): boolean {
  const r = rec(value);
  return r !== null && num(r.planId) && str(r.name) && str(r.subject) && num(r.count);
}

function isAppOffStat(value: unknown): boolean {
  const r = rec(value);
  return r !== null && num(r.sleepMs) && num(r.forcedQuitMs) && num(r.forcedQuitCount) && num(r.plannedOverlapMs);
}

function isReleaseUsage(value: unknown): boolean {
  const r = rec(value);
  return r !== null && num(r.emergencyPass) && num(r.skipToday) && num(r.earlyRelease) && num(r.abortedRuns);
}

/** Seven rows of 24 counts. */
export function isHeatmap(value: unknown): value is number[][] {
  return Array.isArray(value) && value.length === 7 && value.every((row) => Array.isArray(row) && row.length === 24 && row.every(num));
}

export function isWeakness(value: unknown): value is Weakness {
  const r = rec(value);
  return (
    r !== null &&
    num(r.fromMs) &&
    num(r.toMs) &&
    listOf(r.deviationApps, isAppCount) &&
    numOrNull(r.avgFirstAwayMs) &&
    num(r.setsWithAway) &&
    num(r.setsTotal) &&
    isHeatmap(r.heatmap) &&
    listOf(r.postponedPlans, isPostponedPlan) &&
    isAppOffStat(r.appOff) &&
    isReleaseUsage(r.releases)
  );
}

export const dashboardCommands = {
  getDashboardToday: () => invoke<TodayDashboard>('get_dashboard_today'),
  getDashboardWeek: () => invoke<WeekDashboard>('get_dashboard_week'),
  getWeakness: () => invoke<Weakness>('get_weakness'),
};
