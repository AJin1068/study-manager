// Mirror of src-tauri/src/commands/settings.rs. Types must match the Rust serde output.
// UpdateOutcome has a shared fixture (src/api/fixtures/update-outcome.json, checked on both sides).

import { invoke } from '@tauri-apps/api/core';

export type SettingKey = 'autostart' | 'forceQuitAllowed' | 'recheckPlanOnWake' | 'relaunchOnKill' | 'graceWaitSec' | 'emergencyPassDailyLimit' | 'calendarIoEnabled';

/** Mirrors Rust `SettingValue` (untagged): JSON boolean, integer, or string. */
export type SettingValue = boolean | number | string;

export type UpdateOutcome =
  | { outcome: 'applied'; key: string }
  | { outcome: 'rejectedLocked'; key: string };

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
  /** Controls only whether the UI shows the wake re-check; the engine always reconciles. */
  recheckPlanOnWake: boolean;
  /** Relaunch after a kill or crash (macOS only; needs autostart). Default on. */
  relaunchOnKill: boolean;
}

export function isGeneralSettings(value: unknown): value is GeneralSettings {
  if (typeof value !== 'object' || value === null) return false;
  const record = value as Record<string, unknown>;
  return (
    typeof record.autostart === 'boolean' &&
    typeof record.forceQuitAllowed === 'boolean' &&
    typeof record.recheckPlanOnWake === 'boolean' &&
    typeof record.relaunchOnKill === 'boolean'
  );
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

export const settingsCommands = {
  getSettings: () => invoke<GeneralSettings>('get_settings'),
  updateSetting: (key: SettingKey, value: SettingValue) =>
    invoke<UpdateOutcome>('update_setting', { key, value }),
  getSettingHistory: (limit?: number) =>
    invoke<HistoryEntry[]>('get_setting_history', { limit: limit ?? null }),
};
