// Mirror of src-tauri/src/commands/{downtime,power}.rs and the `resumed` event.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface DowntimeRecord {
  id: number;
  kind: 'sleep' | 'forced_quit';
  startAtMs: number;
  endAtMs: number;
  estimated: boolean;
}

export interface DowntimeSummary {
  sleepMs: number;
  /** Time the app was killed (and not running) inside the window. */
  forcedQuitMs: number;
  forcedQuitCount: number;
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

export const downtimeCommands = {
  getDowntimeRecords: (limit?: number) =>
    invoke<DowntimeRecord[]>('get_downtime_records', { limit: limit ?? null }),
  getDowntimeSummary: (fromMs: number, toMs: number) =>
    invoke<DowntimeSummary>('get_downtime_summary', { fromMs, toMs }),
  getAutostartStatus: () => invoke<AutostartStatus>('get_autostart_status'),
  getPowerStatus: () => invoke<PowerSourceStatus>('get_power_status'),
};

/** Fired after a sleep interval is recorded (measured or estimated). Plan recheck hooks in here. */
export function onResumed(handler: (payload: ResumedPayload) => void): Promise<UnlistenFn> {
  return listen<ResumedPayload>('resumed', (event) => handler(event.payload));
}
