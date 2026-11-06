// Mirror of src-tauri/src/commands/study.rs. Types must match the Rust serde output.

import { invoke } from '@tauri-apps/api/core';
import { isUpdateOutcome, type UpdateOutcome } from './settings';

export type StudyLevel = 'easy' | 'medium' | 'hard';

/** `lockHeld` is true while a planned study run (study plus its breaks) holds the lock. */
export interface LockState {
  lockHeld: boolean;
  changedAtMs: number;
}

export interface ModeConfig {
  level: StudyLevel;
  studyMin: number;
  breakMin: number;
  longBreakMin: number | null;
  setsBeforeLongBreak: number | null;
}

/** Minutes accepted for any mode length (D-9); Rust enforces the same range. */
export const MODE_MINUTES_MIN = 1;
export const MODE_MINUTES_MAX = 240;
/** Easy's long break comes after this many sets; Rust enforces the same range. */
export const SETS_BEFORE_LONG_BREAK_MIN = 2;
export const SETS_BEFORE_LONG_BREAK_MAX = 12;

export function isModeConfig(value: unknown): value is ModeConfig {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  const nullableNumber = (v: unknown) => v === null || typeof v === 'number';
  return (
    (r.level === 'easy' || r.level === 'medium' || r.level === 'hard') &&
    typeof r.studyMin === 'number' &&
    typeof r.breakMin === 'number' &&
    nullableNumber(r.longBreakMin) &&
    nullableNumber(r.setsBeforeLongBreak)
  );
}

/** What a mode change did to today's plans that had not started yet. */
export interface ShiftSummary {
  /** Set only when at least one plan moved; the key for `undoPlanBatch`. */
  batchId: string | null;
  movedCount: number;
  /** The new lengths would run past midnight, so no plan was moved. */
  overflowsMidnight: boolean;
  /** Who chose the new start times: the fixed rule, or the validated LLM layout. */
  source: ShiftSource;
  /** Why the LLM did not decide (`llm_disabled`, `llm_layout_invalid`, ...); null when it did or was not needed. */
  fallbackCode: string | null;
}

export type ShiftSource = 'rule' | 'llm';

/** `shift` is null when the change was rejected. */
export type ModeChange = UpdateOutcome & { shift: ShiftSummary | null };

export function isModeChange(value: unknown): value is ModeChange {
  if (!isUpdateOutcome(value)) return false;
  const shift = (value as unknown as Record<string, unknown>).shift;
  if (shift === null) return true;
  if (typeof shift !== 'object' || shift === undefined) return false;
  const r = shift as Record<string, unknown>;
  return (
    (r.batchId === null || typeof r.batchId === 'string') &&
    typeof r.movedCount === 'number' &&
    typeof r.overflowsMidnight === 'boolean' &&
    (r.source === 'rule' || r.source === 'llm') &&
    (r.fallbackCode === null || typeof r.fallbackCode === 'string')
  );
}

export interface ModeSnapshot {
  today: { date: string; level: StudyLevel; stored: boolean };
  configs: ModeConfig[];
}

export const studyCommands = {
  getLockState: () => invoke<LockState>('get_lock_state'),
  getModeConfig: () => invoke<ModeSnapshot>('get_mode_config'),
  updateModeConfig: (config: ModeConfig) =>
    invoke<ModeChange>('update_mode_config', {
      level: config.level,
      studyMin: config.studyMin,
      breakMin: config.breakMin,
      longBreakMin: config.longBreakMin,
      setsBeforeLongBreak: config.setsBeforeLongBreak,
    }),
  setDailyMode: (level: StudyLevel) => invoke<ModeChange>('set_daily_mode', { level }),
};
