// Mirror of src-tauri/src/commands/timer.rs and the timer events (src-tauri/src/timer/).

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type TimerPhase = 'idle' | 'study' | 'break' | 'longBreak';

export interface NextPlan {
  planId: number;
  name: string;
  subject: string;
  date: string;
  sets: number;
  plannedStartMs: number;
  plannedEndMs: number;
}

/** Derived by the Rust core; the UI only counts down to `plannedEndMs`. */
export interface TimerState {
  phase: TimerPhase;
  runId: number | null;
  segmentId: number | null;
  /** 1-based set of the current block; 0 when idle. */
  setIndex: number;
  setsTotal: number;
  plannedEndMs: number | null;
  lockHeld: boolean;
  /** The core's clock when the state was derived (never behind the highest time it has seen). */
  nowMs: number;
  next: NextPlan | null;
}

export interface EngineHealth {
  degraded: boolean;
  consecutiveFailures: number;
  restarts: number;
  clockJumpMs: number;
}

export interface EngineDegradedPayload {
  degraded: boolean;
  consecutiveFailures: number;
}

/** One minute before a break ends (also shown as a system notification). */
export interface BreakEndingSoon {
  runId: number;
  segmentId: number;
  plannedEndMs: number;
  remainingMs: number;
}

/** Sent each time a study block ends. */
export interface SetSummary {
  runId: number;
  setIndex: number;
  setStudiedMs: number;
  todayStudiedMs: number;
  /** Block attempts during this set (app or site, dry-run or enforce). */
  attemptCount: number;
  /** Recognized study time: study app in front and input not idle. */
  focusCreditedMs: number;
  /** Recognized over studied time, 0 to 100; null when nothing was studied. */
  focusScore: number | null;
  /** Study start to the first switch to a non-study app; null when it never happened. */
  firstAwayAfterMs: number | null;
  /** First such switch to the return to a study app; null when it never happened. */
  returnAfterMs: number | null;
}

/** Sent once when a run reaches a terminal status. */
export interface RunSummary {
  runId: number;
  setsTotal: number;
  runStudiedMs: number;
  todayStudiedMs: number;
  /** Block attempts during the whole run. */
  attemptCount: number;
  focusCreditedMs: number;
  focusScore: number | null;
  status: string;
}

const PHASES: readonly string[] = ['idle', 'study', 'break', 'longBreak'];

function nullableNumber(value: unknown): boolean {
  return value === null || typeof value === 'number';
}

export function isNextPlan(value: unknown): value is NextPlan {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    typeof r.planId === 'number' &&
    typeof r.name === 'string' &&
    typeof r.subject === 'string' &&
    typeof r.date === 'string' &&
    typeof r.sets === 'number' &&
    typeof r.plannedStartMs === 'number' &&
    typeof r.plannedEndMs === 'number'
  );
}

/** Runtime guard for values crossing the Tauri boundary; used by the shared-fixture test. */
export function isTimerState(value: unknown): value is TimerState {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    typeof r.phase === 'string' &&
    PHASES.includes(r.phase) &&
    nullableNumber(r.runId) &&
    nullableNumber(r.segmentId) &&
    typeof r.setIndex === 'number' &&
    typeof r.setsTotal === 'number' &&
    nullableNumber(r.plannedEndMs) &&
    typeof r.lockHeld === 'boolean' &&
    typeof r.nowMs === 'number' &&
    (r.next === null || isNextPlan(r.next))
  );
}

export function isBreakEndingSoon(value: unknown): value is BreakEndingSoon {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    typeof r.runId === 'number' &&
    typeof r.segmentId === 'number' &&
    typeof r.plannedEndMs === 'number' &&
    typeof r.remainingMs === 'number'
  );
}

export function isSetSummary(value: unknown): value is SetSummary {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    typeof r.runId === 'number' &&
    typeof r.setIndex === 'number' &&
    typeof r.setStudiedMs === 'number' &&
    typeof r.todayStudiedMs === 'number' &&
    typeof r.attemptCount === 'number' &&
    typeof r.focusCreditedMs === 'number' &&
    nullableNumber(r.focusScore) &&
    nullableNumber(r.firstAwayAfterMs) &&
    nullableNumber(r.returnAfterMs)
  );
}

export function isRunSummary(value: unknown): value is RunSummary {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    typeof r.runId === 'number' &&
    typeof r.setsTotal === 'number' &&
    typeof r.runStudiedMs === 'number' &&
    typeof r.todayStudiedMs === 'number' &&
    typeof r.attemptCount === 'number' &&
    typeof r.focusCreditedMs === 'number' &&
    nullableNumber(r.focusScore) &&
    typeof r.status === 'string'
  );
}

export const timerCommands = {
  getTimerState: () => invoke<TimerState>('get_timer_state'),
  getEngineHealth: () => invoke<EngineHealth>('get_engine_health'),
};

/** Fired on every phase change and after every wake-up reconcile. */
export function onTimerChanged(handler: (state: TimerState) => void): Promise<UnlistenFn> {
  return listen<TimerState>('timer_changed', (event) => handler(event.payload));
}

/** Fired after the engine re-derived the plan on waking from sleep. */
export function onPlanRechecked(handler: (state: TimerState) => void): Promise<UnlistenFn> {
  return listen<TimerState>('plan_rechecked', (event) => handler(event.payload));
}

/** Fired when automatic transitions stop (or resume); the emergency release still works. */
export function onEngineDegraded(handler: (payload: EngineDegradedPayload) => void): Promise<UnlistenFn> {
  return listen<EngineDegradedPayload>('engine_degraded', (event) => handler(event.payload));
}

/** Fired once, a minute before a break of more than a minute ends. */
export function onBreakEndingSoon(handler: (payload: BreakEndingSoon) => void): Promise<UnlistenFn> {
  return listen<BreakEndingSoon>('break_ending_soon', (event) => handler(event.payload));
}

/** Fired each time a study block ends. */
export function onSetSummary(handler: (payload: SetSummary) => void): Promise<UnlistenFn> {
  return listen<SetSummary>('set_summary', (event) => handler(event.payload));
}

/** Fired when a run completes or is aborted. */
export function onRunSummary(handler: (payload: RunSummary) => void): Promise<UnlistenFn> {
  return listen<RunSummary>('run_summary', (event) => handler(event.payload));
}
