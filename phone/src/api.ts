// The phone UI talks to the Rust shell (phone/src-tauri/src/lib.rs) only through these commands and
// the `phone://view` event. Types are the serde output of phone/src-tauri/src/app.rs and
// phone/core/src/{poller,schedule}.rs; the guards below are checked against sample values in
// api.test.ts. Not run on a device (V5-1, V5-4).

import { addPluginListener, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { isBridgeEvent } from '../../src/api/phoneBlocker';

export const VIEW_EVENT = 'phone://view';

export type FreeReason =
  | 'no_lease'
  | 'lease_expired'
  | 'clock_invalid'
  | 'run_ended'
  | 'emergency_pass'
  | 'skip_today'
  | 'early_release'
  | 'pair_revoked';

export type PhoneLock = { state: 'locked' } | { state: 'free'; reason: FreeReason };
export type TimerDisplay = { state: 'studying'; remaining_ms: number } | { state: 'free' };
export type BlockerStatus =
  | { kind: 'active' }
  | { kind: 'inactive' }
  | { kind: 'service_disabled' }
  | { kind: 'unreachable' }
  | { kind: 'unsupported'; reason: string };

export interface LeaseDiagnostics {
  lock: PhoneLock;
  seq: number | null;
  lock_remaining_ms: number | null;
  timer: TimerDisplay;
  last_server_contact_ago_ms: number | null;
  rejected_leases: number;
}

export interface PollReport {
  diagnostics: LeaseDiagnostics;
  blocker: BlockerStatus;
  service_enabled: boolean;
  online: boolean;
  last_error: string | null;
}

export interface PhoneView {
  paired: boolean;
  pairingLost: boolean;
  report: PollReport | null;
  platform: 'android' | 'ios' | 'other';
  lastError: string | null;
}

export interface ScheduleItem {
  occurrence_key: string;
  start_ms: number;
  plan_name: string;
  subject: string;
  sets: number;
  status: 'planned' | 'skipped';
}

export interface ScheduleView {
  items: { items: ScheduleItem[] };
  source: 'no_feed';
}

export interface Ack {
  code: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

const isNum = (v: unknown): v is number => typeof v === 'number';
const isStr = (v: unknown): v is string => typeof v === 'string';
const isBool = (v: unknown): v is boolean => typeof v === 'boolean';
const orNull = <T>(v: unknown, guard: (x: unknown) => x is T): boolean => v === null || guard(v);

function isPhoneLock(v: unknown): v is PhoneLock {
  return isRecord(v) && (v.state === 'locked' || (v.state === 'free' && isStr(v.reason)));
}

function isTimer(v: unknown): v is TimerDisplay {
  return isRecord(v) && (v.state === 'free' || (v.state === 'studying' && isNum(v.remaining_ms)));
}

function isBlockerStatus(v: unknown): v is BlockerStatus {
  if (!isRecord(v)) return false;
  if (v.kind === 'unsupported') return isStr(v.reason);
  return v.kind === 'active' || v.kind === 'inactive' || v.kind === 'service_disabled' || v.kind === 'unreachable';
}

export function isPollReport(v: unknown): v is PollReport {
  if (!isRecord(v) || !isRecord(v.diagnostics)) return false;
  const d = v.diagnostics;
  return (
    isPhoneLock(d.lock) &&
    orNull(d.seq, isNum) &&
    orNull(d.lock_remaining_ms, isNum) &&
    isTimer(d.timer) &&
    orNull(d.last_server_contact_ago_ms, isNum) &&
    isNum(d.rejected_leases) &&
    isBlockerStatus(v.blocker) &&
    isBool(v.service_enabled) &&
    isBool(v.online) &&
    orNull(v.last_error, isStr)
  );
}

export function isPhoneView(v: unknown): v is PhoneView {
  return (
    isRecord(v) &&
    isBool(v.paired) &&
    isBool(v.pairingLost) &&
    (v.report === null || isPollReport(v.report)) &&
    (v.platform === 'android' || v.platform === 'ios' || v.platform === 'other') &&
    orNull(v.lastError, isStr)
  );
}

export function isAck(v: unknown): v is Ack {
  return isRecord(v) && isStr(v.code);
}

function checked<T>(guard: (v: unknown) => v is T, name: string) {
  return (value: unknown): T => {
    if (!guard(value)) throw new Error(`unexpected ${name} reply`);
    return value;
  };
}

const asView = checked(isPhoneView, 'phone view');
const asAck = checked(isAck, 'ack');

export const phoneApi = {
  view: async () => asView(await invoke('phone_view')),
  pollNow: async () => asView(await invoke('phone_poll_now')),
  /** `text` is exactly what the QR scanner read. */
  pair: async (text: string) => asAck(await invoke('phone_pair', { text })),
  unpair: async () => asAck(await invoke('phone_unpair')),
  schedule: () => invoke<ScheduleView>('phone_schedule'),
};

/** Subscribes to the Rust poll thread's views. Returns the unsubscribe function. */
export async function onView(callback: (view: PhoneView) => void): Promise<() => void> {
  return listen<unknown>(VIEW_EVENT, (event) => {
    if (isPhoneView(event.payload)) callback(event.payload);
  });
}

/** Forwards the Kotlin plugin's events (`service_disabled`, `service_enabled`, `lock_ended`) to Rust. */
export async function forwardBlockerEvents(): Promise<() => Promise<void>> {
  const listener = await addPluginListener('blocker', 'blocker', (payload: unknown) => {
    if (isBridgeEvent(payload)) void invoke('phone_blocker_event', { event: payload });
  });
  return () => listener.unregister();
}

interface ScanResult {
  content: string;
}

/** Camera scan through tauri-plugin-barcode-scanner (registered on mobile only). */
export async function scanQr(): Promise<string> {
  await invoke('plugin:barcode-scanner|request_permissions');
  const result = await invoke<ScanResult>('plugin:barcode-scanner|scan', { windowed: false, formats: ['QR_CODE'] });
  if (!isRecord(result) || !isStr(result.content)) throw new Error('unexpected scan reply');
  return result.content;
}
