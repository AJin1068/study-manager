// Mirror of the Rust command surface (src-tauri/src/commands.rs and settings/study/downtime).
// The UI talks to the Rust core only through these commands and the `resumed` event.
// Types here must match the Rust serde output. Only UpdateOutcome has a shared fixture today
// (src/api/fixtures/update-outcome.json, checked on both sides). Other shapes are not contract-tested.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type SettingKey = 'autostart' | 'forceQuitAllowed';

export type UpdateOutcome =
  | { outcome: 'applied'; key: string }
  | { outcome: 'rejectedLocked'; key: string };

export type StudyLevel = 'easy' | 'medium' | 'hard';

/** Runtime guard for values crossing the Tauri boundary. Used by the shared-fixture test. */
export function isUpdateOutcome(value: unknown): value is UpdateOutcome {
  if (typeof value !== 'object' || value === null) return false;
  const record = value as Record<string, unknown>;
  if (typeof record.key !== 'string') return false;
  return record.outcome === 'applied' || record.outcome === 'rejectedLocked';
}

export interface GeneralSettings {
  autostart: boolean;
  forceQuitAllowed: boolean;
}

export interface HistoryEntry {
  id: number;
  atMs: number;
  key: string;
  before: string | null;
  after: string;
  reason: string;
  outcome: string;
}

export interface StudyState {
  studying: boolean;
  changedAtMs: number;
}

export interface ModeConfig {
  level: StudyLevel;
  studyMin: number;
  breakMin: number;
  longBreakMin: number | null;
  setsBeforeLongBreak: number | null;
}

export interface ModeSnapshot {
  today: { date: string; level: StudyLevel; stored: boolean };
  configs: ModeConfig[];
}

export interface DowntimeRecord {
  id: number;
  kind: 'sleep' | 'forced_quit';
  startAtMs: number;
  endAtMs: number;
  estimated: boolean;
}

export interface AutostartStatus {
  enabled: boolean;
}

export interface PowerSourceStatus {
  active: boolean;
  reason: string | null;
}

export interface ResumedPayload {
  startAtMs: number;
  endAtMs: number;
  estimated: boolean;
}

export const commands = {
  getSettings: () => invoke<GeneralSettings>('get_settings'),
  updateSetting: (key: SettingKey, value: boolean) =>
    invoke<UpdateOutcome>('update_setting', { key, value }),
  getSettingHistory: (limit?: number) =>
    invoke<HistoryEntry[]>('get_setting_history', { limit: limit ?? null }),
  getStudyState: () => invoke<StudyState>('get_study_state'),
  setStudyStateTest: (studying: boolean) =>
    invoke<UpdateOutcome>('set_study_state_test', { studying }),
  getModeConfig: () => invoke<ModeSnapshot>('get_mode_config'),
  setDailyMode: (level: StudyLevel) => invoke<UpdateOutcome>('set_daily_mode', { level }),
  getDowntimeRecords: (limit?: number) =>
    invoke<DowntimeRecord[]>('get_downtime_records', { limit: limit ?? null }),
  getAutostartStatus: () => invoke<AutostartStatus>('get_autostart_status'),
  getPowerStatus: () => invoke<PowerSourceStatus>('get_power_status'),
};

/** Fired after a sleep interval is recorded (measured or estimated). Plan recheck hooks in here. */
export function onResumed(handler: (payload: ResumedPayload) => void): Promise<UnlistenFn> {
  return listen<ResumedPayload>('resumed', (event) => handler(event.payload));
}
