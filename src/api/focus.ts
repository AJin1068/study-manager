// Mirror of src-tauri/src/commands/focus.rs. Types must match the Rust serde output.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { UpdateOutcome } from './settings';

/** One app that counts as studying. `appId` is a bundle id (macOS) or normalized executable path. */
export interface StudyApp {
  id: number;
  kind: string;
  appId: string;
  enabled: boolean;
  createdAtMs: number;
}

export type TitleAccess = 'granted' | 'denied' | 'unsupported';

export interface FocusSetup {
  studyAppCount: number;
  /** Show "공부 앱을 선택하세요": no enabled study app, so no time can be recognized. */
  needsStudyApps: boolean;
  /** Window titles are not read or stored in stage 1; this only drives the explanation screen. */
  titleAccess: TitleAccess;
  /** The foreground and idle sensors answer on this machine. */
  sensorsAvailable: boolean;
}

const TITLE_ACCESS: readonly string[] = ['granted', 'denied', 'unsupported'];

export function isStudyApp(value: unknown): value is StudyApp {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    typeof r.id === 'number' &&
    typeof r.kind === 'string' &&
    typeof r.appId === 'string' &&
    typeof r.enabled === 'boolean' &&
    typeof r.createdAtMs === 'number'
  );
}

export function isFocusSetup(value: unknown): value is FocusSetup {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    typeof r.studyAppCount === 'number' &&
    typeof r.needsStudyApps === 'boolean' &&
    typeof r.titleAccess === 'string' &&
    TITLE_ACCESS.includes(r.titleAccess) &&
    typeof r.sensorsAvailable === 'boolean'
  );
}

export type IgnoreReason = 'notStudying' | 'tooSoon';

/** Result of a "just zoned out" press or a check-in answer. Only `recorded` wrote a row. */
export type SelfReportOutcome =
  | { status: 'recorded' }
  | { status: 'acknowledged' }
  | { status: 'ignored'; reason: IgnoreReason };

/** Whether the global shortcut could be registered; the in-app button works either way. */
export interface SelfReportStatus {
  shortcut: string;
  shortcutRegistered: boolean;
  shortcutError: string | null;
}

/** A random check-in waiting for an answer ("were you focused just now?"). */
export interface CheckinPrompt {
  runId: number;
  segmentId: number;
  askedAtMs: number;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

export function isSelfReportOutcome(value: unknown): value is SelfReportOutcome {
  if (!isRecord(value)) return false;
  if (value.status === 'recorded' || value.status === 'acknowledged') return true;
  return value.status === 'ignored' && (value.reason === 'notStudying' || value.reason === 'tooSoon');
}

export function isSelfReportStatus(value: unknown): value is SelfReportStatus {
  if (!isRecord(value)) return false;
  return (
    typeof value.shortcut === 'string' &&
    typeof value.shortcutRegistered === 'boolean' &&
    (value.shortcutError === null || typeof value.shortcutError === 'string')
  );
}

export function isCheckinPrompt(value: unknown): value is CheckinPrompt {
  if (!isRecord(value)) return false;
  return typeof value.runId === 'number' && typeof value.segmentId === 'number' && typeof value.askedAtMs === 'number';
}

export const focusCommands = {
  listStudyApps: () => invoke<StudyApp[]>('list_study_apps'),
  addStudyApp: (appId: string) => invoke<UpdateOutcome>('add_study_app', { appId }),
  removeStudyApp: (id: number) => invoke<UpdateOutcome>('remove_study_app', { id }),
  setStudyAppEnabled: (id: number, enabled: boolean) => invoke<UpdateOutcome>('set_study_app_enabled', { id, enabled }),
  getFocusSetup: () => invoke<FocusSetup>('get_focus_setup'),
  reportDistraction: () => invoke<SelfReportOutcome>('report_distraction'),
  getSelfReportStatus: () => invoke<SelfReportStatus>('get_self_report_status'),
  getPendingCheckin: () => invoke<CheckinPrompt | null>('get_pending_checkin'),
  /** `focused: false` records a self report; `true` only acknowledges. */
  answerCheckin: (focused: boolean) => invoke<SelfReportOutcome>('answer_checkin', { focused }),
};

/** Fired when a random check-in falls due. */
export function onFocusCheckin(handler: (prompt: CheckinPrompt) => void): Promise<UnlistenFn> {
  return listen<CheckinPrompt>('focus_checkin', (event) => handler(event.payload));
}

/** Fired when the global shortcut was pressed; the payload says whether a row was written. */
export function onSelfReportRecorded(handler: (outcome: SelfReportOutcome) => void): Promise<UnlistenFn> {
  return listen<SelfReportOutcome>('self_report_recorded', (event) => handler(event.payload));
}
