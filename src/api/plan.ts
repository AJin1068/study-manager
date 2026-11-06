// Mirror of src-tauri/src/commands/plan.rs. Types must match the Rust serde output; the shared
// fixture src/api/fixtures/plan-contract.json is checked on both sides.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { StudyLevel } from './study';
import { isUpdateOutcome, type UpdateOutcome } from './settings';

/** Reasons a user may pick when editing; the rest are written by the app itself. */
export type UserReasonCode = 'postponed' | 'sick' | 'custom';
export type PlanScope = 'occurrence' | 'series';

/** Reasons and actions the app writes; the DB reserves the chatbot/LLM values for stage 3. */
export type PlanAction =
  | 'create'
  | 'move'
  | 'retime'
  | 'resize_sets'
  | 'delete'
  | 'repeat_change'
  | 'mode_shift'
  | 'undo'
  | 'chatbot_create'
  | 'llm_adjust';
export type PlanReasonCode =
  | 'created'
  | 'postponed'
  | 'sick'
  | 'custom'
  | 'mode_shift'
  | 'undo'
  | 'chatbot'
  | 'llm_adjust';

/** Weekday bitmask: bit 0 = Monday .. bit 6 = Sunday. */
export interface Repeat {
  weekdays: number;
  until: string | null;
}

export interface CreatePlanInput {
  name: string;
  subject: string;
  /** Local `YYYY-MM-DD`. */
  startDate: string;
  /** Local `HH:MM`. */
  startTime: string;
  sets: number;
  repeat: Repeat | null;
}

export interface UpdatePlanInput {
  planId: number;
  scope: PlanScope;
  /** The occurrence's original date (`sourceDate` of the listed occurrence). */
  date: string;
  /** Occurrence scope only. */
  newDate: string | null;
  newTime: string | null;
  newSets: number | null;
  /** Series scope only. */
  repeat: Repeat | null;
  reasonCode: UserReasonCode;
  reasonText: string | null;
}

export interface DeletePlanInput {
  planId: number;
  scope: PlanScope;
  date: string;
  reasonCode: UserReasonCode;
  reasonText: string | null;
}

export interface PlanChange {
  outcome: UpdateOutcome;
  planId: number | null;
}

export interface Occurrence {
  planId: number;
  seriesId: number;
  name: string;
  subject: string;
  date: string;
  sourceDate: string;
  startTime: string;
  sets: number;
  moved: boolean;
  level: StudyLevel;
  durationMin: number;
  plannedStartMs: number;
  plannedEndMs: number;
  eligibleForMissed: boolean;
}

export interface PlanHistoryEntry {
  id: number;
  planId: number | null;
  atMs: number;
  action: PlanAction;
  reasonCode: PlanReasonCode;
  reasonText: string | null;
  beforeJson: string | null;
  afterJson: string | null;
  outcome: 'applied' | 'rejected_locked';
  batchId: string | null;
}

export interface SubjectCount {
  subject: string;
  count: number;
}

export interface PlanStats {
  month: string;
  postponedCount: number;
  bySubject: SubjectCount[];
}

/** Runtime guard for values crossing the Tauri boundary. Used by the shared-fixture test. */
export function isPlanChange(value: unknown): value is PlanChange {
  if (typeof value !== 'object' || value === null) return false;
  const record = value as Record<string, unknown>;
  return (
    isUpdateOutcome(record.outcome) &&
    (record.planId === null || typeof record.planId === 'number')
  );
}

export function isOccurrence(value: unknown): value is Occurrence {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    typeof r.planId === 'number' &&
    typeof r.seriesId === 'number' &&
    typeof r.name === 'string' &&
    typeof r.subject === 'string' &&
    typeof r.date === 'string' &&
    typeof r.sourceDate === 'string' &&
    typeof r.startTime === 'string' &&
    typeof r.sets === 'number' &&
    typeof r.moved === 'boolean' &&
    (r.level === 'easy' || r.level === 'medium' || r.level === 'hard') &&
    typeof r.durationMin === 'number' &&
    typeof r.plannedStartMs === 'number' &&
    typeof r.plannedEndMs === 'number' &&
    typeof r.eligibleForMissed === 'boolean'
  );
}

const PLAN_ACTIONS: readonly string[] = [
  'create', 'move', 'retime', 'resize_sets', 'delete', 'repeat_change', 'mode_shift', 'undo',
  'chatbot_create', 'llm_adjust',
];
const PLAN_REASON_CODES: readonly string[] = [
  'created', 'postponed', 'sick', 'custom', 'mode_shift', 'undo', 'chatbot', 'llm_adjust',
];

function nullableString(value: unknown): boolean {
  return value === null || typeof value === 'string';
}

export function isPlanHistoryEntry(value: unknown): value is PlanHistoryEntry {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    typeof r.id === 'number' &&
    (r.planId === null || typeof r.planId === 'number') &&
    typeof r.atMs === 'number' &&
    typeof r.action === 'string' &&
    PLAN_ACTIONS.includes(r.action) &&
    typeof r.reasonCode === 'string' &&
    PLAN_REASON_CODES.includes(r.reasonCode) &&
    nullableString(r.reasonText) &&
    nullableString(r.beforeJson) &&
    nullableString(r.afterJson) &&
    (r.outcome === 'applied' || r.outcome === 'rejected_locked') &&
    nullableString(r.batchId)
  );
}

export function isPlanStats(value: unknown): value is PlanStats {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    typeof r.month === 'string' &&
    typeof r.postponedCount === 'number' &&
    Array.isArray(r.bySubject) &&
    r.bySubject.every((entry: unknown) => {
      if (typeof entry !== 'object' || entry === null) return false;
      const e = entry as Record<string, unknown>;
      return typeof e.subject === 'string' && typeof e.count === 'number';
    })
  );
}

export type IcsSkipReason = 'allDay' | 'timezone' | 'unsupportedRule' | 'invalid' | 'past' | 'overlap';

export interface IcsSkipCount {
  reason: IcsSkipReason;
  count: number;
}

export interface IcsPreviewItem {
  name: string;
  subject: string;
  startDate: string;
  startTime: string;
  sets: number;
  repeat: Repeat | null;
  /** Occurrences the file removed with EXDATE. */
  excludedDates: number;
}

export interface IcsPreview {
  items: IcsPreviewItem[];
  skipped: IcsSkipCount[];
  skippedTotal: number;
}

export interface IcsApplyResult {
  change: PlanChange;
  /** Undo handle; null when nothing was imported. */
  batchId: string | null;
  imported: number;
  skipped: IcsSkipCount[];
  skippedTotal: number;
}

export interface IcsExport {
  content: string;
  eventCount: number;
}

const ICS_SKIP_REASONS: readonly string[] = ['allDay', 'timezone', 'unsupportedRule', 'invalid', 'past', 'overlap'];

function isIcsSkipCounts(value: unknown): value is IcsSkipCount[] {
  return (
    Array.isArray(value) &&
    value.every((entry: unknown) => {
      if (typeof entry !== 'object' || entry === null) return false;
      const e = entry as Record<string, unknown>;
      return typeof e.reason === 'string' && ICS_SKIP_REASONS.includes(e.reason) && typeof e.count === 'number';
    })
  );
}

function isRepeat(value: unknown): value is Repeat {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return typeof r.weekdays === 'number' && nullableString(r.until);
}

function isIcsPreviewItem(value: unknown): value is IcsPreviewItem {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    typeof r.name === 'string' &&
    typeof r.subject === 'string' &&
    typeof r.startDate === 'string' &&
    typeof r.startTime === 'string' &&
    typeof r.sets === 'number' &&
    typeof r.excludedDates === 'number' &&
    (r.repeat === null || isRepeat(r.repeat))
  );
}

export function isIcsPreview(value: unknown): value is IcsPreview {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    Array.isArray(r.items) &&
    r.items.every(isIcsPreviewItem) &&
    isIcsSkipCounts(r.skipped) &&
    typeof r.skippedTotal === 'number'
  );
}

export function isIcsApplyResult(value: unknown): value is IcsApplyResult {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    isPlanChange(r.change) &&
    nullableString(r.batchId) &&
    typeof r.imported === 'number' &&
    isIcsSkipCounts(r.skipped) &&
    typeof r.skippedTotal === 'number'
  );
}

export function isIcsExport(value: unknown): value is IcsExport {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return typeof r.content === 'string' && typeof r.eventCount === 'number';
}

export const planCommands = {
  createPlan: (input: CreatePlanInput) => invoke<PlanChange>('create_plan', { input }),
  updatePlan: (input: UpdatePlanInput) => invoke<PlanChange>('update_plan', { input }),
  deletePlan: (input: DeletePlanInput) => invoke<PlanChange>('delete_plan', { input }),
  /** Reverts a mode-shift batch (plans and the mode that caused them). */
  undoPlanBatch: (batchId: string) => invoke<PlanChange>('undo_plan_batch', { batchId }),
  /** Inclusive `YYYY-MM-DD` range, at most 400 days. */
  listOccurrences: (from: string, to: string) =>
    invoke<Occurrence[]>('list_occurrences', { from, to }),
  /** With a plan id: the timeline of that plan's whole series. */
  getPlanHistory: (planId?: number, limit?: number) =>
    invoke<PlanHistoryEntry[]>('get_plan_history', { planId: planId ?? null, limit: limit ?? null }),
  /** `month` is a local `YYYY-MM`. */
  getPlanStats: (month: string) => invoke<PlanStats>('get_plan_stats', { month }),
  /** Calendar import/export is off until the `calendarIoEnabled` setting is on. */
  getCalendarIoEnabled: () => invoke<boolean>('get_calendar_io_enabled'),
  /** The `.ics` text of every live plan; the UI saves it through the user's own save action. */
  exportPlansIcs: () => invoke<IcsExport>('export_plans_ics'),
  /** What `applyIcsImport` would create from this file text; changes nothing. */
  previewIcsImport: (content: string) => invoke<IcsPreview>('preview_ics_import', { content }),
  applyIcsImport: (content: string) => invoke<IcsApplyResult>('apply_ics_import', { content }),
  /** Reverts one import batch (`IcsApplyResult.batchId`). */
  undoIcsImport: (batchId: string) => invoke<PlanChange>('undo_ics_import', { batchId }),
};

/** Fired after an applied plan change; refetch occurrences and history. */
export function onPlansChanged(handler: () => void): Promise<UnlistenFn> {
  return listen('plans_changed', () => handler());
}
