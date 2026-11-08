import type {
  CreatePlanInput,
  DeletePlanInput,
  Occurrence,
  PlanAction,
  PlanChange,
  PlanHistoryEntry,
  PlanReasonCode,
  PlanScope,
  Repeat,
  UpdatePlanInput,
  UserReasonCode,
} from '../api';

// Mirrors the Rust limits in plan.rs; the core stays the authority, this is early feedback.
export const MAX_SETS = 24;
export const MAX_TEXT_CHARS = 100;

const DAY_MS = 86_400_000;
const DATE_RE = /^(\d{4})-(\d{2})-(\d{2})$/;
const TIME_RE = /^([01]\d|2[0-3]):([0-5]\d)$/;

// Dates are plain `YYYY-MM-DD` strings. All arithmetic goes through UTC midnight so the host
// timezone and DST never shift a calendar day.
function toUtcMs(date: string): number {
  const m = DATE_RE.exec(date);
  if (m === null) throw new Error(`invalid date: ${date}`);
  return Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
}

function fromUtcMs(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}

export function isValidDate(date: string): boolean {
  const m = DATE_RE.exec(date);
  if (m === null) return false;
  return fromUtcMs(toUtcMs(date)) === date;
}

export function isValidTime(time: string): boolean {
  return TIME_RE.test(time);
}

export function addDays(date: string, days: number): string {
  return fromUtcMs(toUtcMs(date) + days * DAY_MS);
}

/** 0 = Monday .. 6 = Sunday, matching the Rust weekday bitmask. */
export function weekdayIndex(date: string): number {
  return (new Date(toUtcMs(date)).getUTCDay() + 6) % 7;
}

export function weekStart(date: string): string {
  return addDays(date, -weekdayIndex(date));
}

export function weekDates(date: string): string[] {
  const start = weekStart(date);
  return Array.from({ length: 7 }, (_, i) => addDays(start, i));
}

/** `YYYY-MM` of a date. */
export function monthKey(date: string): string {
  return date.slice(0, 7);
}

export function monthFirstDay(date: string): string {
  return `${monthKey(date)}-01`;
}

export function addMonths(date: string, months: number): string {
  const [year, month] = [Number(date.slice(0, 4)), Number(date.slice(5, 7))];
  const index = year * 12 + (month - 1) + months;
  const y = Math.floor(index / 12);
  const mo = (index % 12) + 1;
  return `${String(y).padStart(4, '0')}-${String(mo).padStart(2, '0')}-01`;
}

/** Whole weeks (Monday first) that cover the month containing `date`. */
export function monthGrid(date: string): string[][] {
  const first = monthFirstDay(date);
  const next = addMonths(first, 1);
  const lastDay = addDays(next, -1);
  const weeks: string[][] = [];
  for (let start = weekStart(first); start <= lastDay; start = addDays(start, 7)) {
    weeks.push(weekDates(start));
  }
  return weeks;
}

export type CalendarView = 'week' | 'month';

/** Inclusive range to request from `list_occurrences` for the visible grid. */
export function visibleRange(view: CalendarView, anchor: string): { from: string; to: string } {
  if (view === 'week') {
    const days = weekDates(anchor);
    return { from: days[0] ?? anchor, to: days[6] ?? anchor };
  }
  const weeks = monthGrid(anchor);
  const first = weeks[0]?.[0] ?? anchor;
  const last = weeks[weeks.length - 1]?.[6] ?? anchor;
  return { from: first, to: last };
}

export function shiftAnchor(view: CalendarView, anchor: string, direction: -1 | 1): string {
  return view === 'week' ? addDays(anchor, 7 * direction) : addMonths(anchor, direction);
}

export function groupByDate(occurrences: Occurrence[]): Map<string, Occurrence[]> {
  const groups = new Map<string, Occurrence[]>();
  for (const occurrence of occurrences) {
    const list = groups.get(occurrence.date);
    if (list === undefined) groups.set(occurrence.date, [occurrence]);
    else list.push(occurrence);
  }
  for (const list of groups.values()) list.sort((a, b) => a.startTime.localeCompare(b.startTime));
  return groups;
}

/** "20:00~23:00"; an end that falls on a later day gets "+1일" (plans may cross midnight). */
export function derivedEnd(startTime: string, durationMin: number): string {
  const m = TIME_RE.exec(startTime);
  if (m === null) return startTime;
  const total = Number(m[1]) * 60 + Number(m[2]) + durationMin;
  const dayShift = Math.floor(total / 1440);
  const inDay = total % 1440;
  const hh = String(Math.floor(inDay / 60)).padStart(2, '0');
  const mm = String(inDay % 60).padStart(2, '0');
  return `${startTime}~${hh}:${mm}${dayShift > 0 ? ` (+${dayShift}일)` : ''}`;
}

export const WEEKDAY_LABELS = ['월', '화', '수', '목', '금', '토', '일'] as const;

export function weekdaysLabel(mask: number): string {
  return WEEKDAY_LABELS.filter((_, i) => (mask & (1 << i)) !== 0).join('·');
}

export function toggleWeekday(mask: number, index: number): number {
  return mask ^ (1 << index);
}

export const USER_REASONS: ReadonlyArray<{ code: UserReasonCode; label: string }> = [
  { code: 'postponed', label: '미뤘다' },
  { code: 'sick', label: '아팠다' },
  { code: 'custom', label: '기타(직접 적기)' },
];

const REASON_LABEL: Record<PlanReasonCode, string> = {
  created: '새로 만듦',
  postponed: '미뤘다',
  sick: '아팠다',
  custom: '기타',
  mode_shift: '모드 변경에 따른 이동',
  undo: '되돌림',
  chatbot: '챗봇',
  llm_adjust: '자동 조정',
};

const ACTION_LABEL: Record<PlanAction, string> = {
  create: '만들기',
  move: '옮기기',
  retime: '시간 변경',
  resize_sets: '세트 수 변경',
  delete: '삭제',
  repeat_change: '반복 변경',
  mode_shift: '모드에 따른 이동',
  undo: '되돌리기',
  chatbot_create: '챗봇으로 만들기',
  llm_adjust: '자동 조정',
};

export function reasonLabel(code: PlanReasonCode): string {
  return REASON_LABEL[code];
}

export function actionLabel(action: PlanAction): string {
  return ACTION_LABEL[action];
}

export interface ReasonDraft {
  /** null until the user picks one: submitting without a reason is not allowed (PLAN D-7). */
  code: UserReasonCode | null;
  text: string;
}

export type Validated<T> = { ok: true; value: T } | { ok: false; error: string };

export function validateReason(
  draft: ReasonDraft,
): Validated<{ reasonCode: UserReasonCode; reasonText: string | null }> {
  if (draft.code === null) return { ok: false, error: '이유를 선택하세요.' };
  const text = draft.text.trim();
  if (draft.code === 'custom' && text === '') return { ok: false, error: '이유를 적어주세요.' };
  if ([...text].length > MAX_TEXT_CHARS) {
    return { ok: false, error: `이유는 ${MAX_TEXT_CHARS}자 이하로 적어주세요.` };
  }
  return { ok: true, value: { reasonCode: draft.code, reasonText: text === '' ? null : text } };
}

function validateSets(raw: string): Validated<number> {
  const text = raw.trim();
  if (!/^\d+$/.test(text)) return { ok: false, error: '세트 수는 정수로 입력하세요.' };
  const sets = Number(text);
  if (sets < 1 || sets > MAX_SETS) return { ok: false, error: `세트 수는 1~${MAX_SETS} 사이입니다.` };
  return { ok: true, value: sets };
}

function validateRepeat(weekdays: number, until: string, startDate: string): Validated<Repeat | null> {
  if (weekdays === 0) return { ok: true, value: null };
  if ((weekdays & (1 << weekdayIndex(startDate))) === 0) {
    return { ok: false, error: '반복 요일에 시작 날짜의 요일이 들어 있어야 합니다.' };
  }
  const untilText = until.trim();
  if (untilText === '') return { ok: true, value: { weekdays, until: null } };
  if (!isValidDate(untilText)) return { ok: false, error: '반복 종료일 형식이 맞지 않습니다.' };
  if (untilText < startDate) return { ok: false, error: '반복 종료일이 시작 날짜보다 앞입니다.' };
  return { ok: true, value: { weekdays, until: untilText } };
}

export interface CreateDraft {
  name: string;
  subject: string;
  startDate: string;
  startTime: string;
  sets: string;
  /** 0 = no repeat. */
  weekdays: number;
  repeatUntil: string;
}

function validateText(raw: string, label: string): Validated<string> {
  const text = raw.trim();
  if (text === '') return { ok: false, error: `${label}을(를) 입력하세요.` };
  if ([...text].length > MAX_TEXT_CHARS) {
    return { ok: false, error: `${label}은(는) ${MAX_TEXT_CHARS}자 이하로 적어주세요.` };
  }
  return { ok: true, value: text };
}

export function buildCreateInput(draft: CreateDraft): Validated<CreatePlanInput> {
  const name = validateText(draft.name, '제목');
  if (!name.ok) return name;
  const subject = validateText(draft.subject, '과목');
  if (!subject.ok) return subject;
  if (!isValidDate(draft.startDate)) return { ok: false, error: '시작 날짜 형식이 맞지 않습니다.' };
  if (!isValidTime(draft.startTime)) return { ok: false, error: '시작 시각 형식이 맞지 않습니다.' };
  const sets = validateSets(draft.sets);
  if (!sets.ok) return sets;
  const repeat = validateRepeat(draft.weekdays, draft.repeatUntil, draft.startDate);
  if (!repeat.ok) return repeat;
  return {
    ok: true,
    value: {
      name: name.value,
      subject: subject.value,
      startDate: draft.startDate,
      startTime: draft.startTime,
      sets: sets.value,
      repeat: repeat.value,
    },
  };
}

export interface EditDraft {
  scope: PlanScope;
  newDate: string;
  newTime: string;
  sets: string;
  weekdays: number;
  repeatUntil: string;
  reason: ReasonDraft;
}

/** Fields are sent only when they differ from the listed occurrence, so "nothing changed" is caught early. */
export function buildUpdateInput(occurrence: Occurrence, draft: EditDraft): Validated<UpdatePlanInput> {
  const reason = validateReason(draft.reason);
  if (!reason.ok) return reason;
  if (!isValidTime(draft.newTime)) return { ok: false, error: '시작 시각 형식이 맞지 않습니다.' };
  const sets = validateSets(draft.sets);
  if (!sets.ok) return sets;
  const newTime = draft.newTime === occurrence.startTime ? null : draft.newTime;
  const newSets = sets.value === occurrence.sets ? null : sets.value;

  let newDate: string | null = null;
  let repeat: Repeat | null = null;
  if (draft.scope === 'occurrence') {
    if (!isValidDate(draft.newDate)) return { ok: false, error: '날짜 형식이 맞지 않습니다.' };
    newDate = draft.newDate === occurrence.date ? null : draft.newDate;
  } else {
    const parsed = validateRepeat(draft.weekdays, draft.repeatUntil, occurrence.sourceDate);
    if (!parsed.ok) return parsed;
    repeat = parsed.value;
  }
  if (newDate === null && newTime === null && newSets === null && repeat === null) {
    return { ok: false, error: '바뀐 내용이 없습니다.' };
  }
  return {
    ok: true,
    value: {
      planId: occurrence.planId,
      scope: draft.scope,
      date: occurrence.sourceDate,
      newDate,
      newTime,
      newSets,
      repeat,
      ...reason.value,
    },
  };
}

export function buildDeleteInput(
  occurrence: Occurrence,
  scope: PlanScope,
  reasonDraft: ReasonDraft,
): Validated<DeletePlanInput> {
  const reason = validateReason(reasonDraft);
  if (!reason.ok) return reason;
  return {
    ok: true,
    value: { planId: occurrence.planId, scope, date: occurrence.sourceDate, ...reason.value },
  };
}

/** Message after a change attempt. A lock rejection is not an error: the core recorded it. */
export function planChangeMessage(change: PlanChange): { text: string; rejected: boolean } {
  if (change.outcome.outcome === 'rejectedLocked') {
    return {
      text: '계획된 공부 진행 중이라 계획을 바꾸지 않았습니다. 시도는 기록에 남았습니다.',
      rejected: true,
    };
  }
  return { text: '계획을 저장했습니다.', rejected: false };
}

interface SnapshotFields {
  date?: string;
  time?: string;
  sets?: number;
}

function parseSnapshot(json: string | null): SnapshotFields | null {
  if (json === null) return null;
  try {
    const value: unknown = JSON.parse(json);
    if (typeof value !== 'object' || value === null) return null;
    const r = value as Record<string, unknown>;
    const out: SnapshotFields = {};
    const date = typeof r.date === 'string' ? r.date : typeof r.startDate === 'string' ? r.startDate : undefined;
    const time = typeof r.time === 'string' ? r.time : typeof r.startTime === 'string' ? r.startTime : undefined;
    if (date !== undefined) out.date = date;
    if (time !== undefined) out.time = time;
    if (typeof r.sets === 'number') out.sets = r.sets;
    return out;
  } catch {
    return null;
  }
}

function describeSnapshot(s: SnapshotFields): string {
  const parts: string[] = [];
  if (s.date !== undefined) parts.push(s.date);
  if (s.time !== undefined) parts.push(s.time);
  if (s.sets !== undefined) parts.push(`${s.sets}세트`);
  return parts.join(' ');
}

/** One-line summary of a history row: "옮기기: 2026-03-02 20:00 3세트 → 2026-03-09 20:00 3세트". */
export function describeHistoryEntry(entry: PlanHistoryEntry): string {
  const head = entry.outcome === 'rejected_locked' ? `${actionLabel(entry.action)} (거절됨: 잠금 중)` : actionLabel(entry.action);
  const before = parseSnapshot(entry.beforeJson);
  const after = parseSnapshot(entry.afterJson);
  const beforeText = before === null ? '' : describeSnapshot(before);
  const afterText = after === null ? '' : describeSnapshot(after);
  if (beforeText !== '' && afterText !== '') return `${head}: ${beforeText} → ${afterText}`;
  if (afterText !== '') return `${head}: ${afterText}`;
  if (beforeText !== '') return `${head}: ${beforeText}`;
  return head;
}

export function formatAt(atMs: number): string {
  const d = new Date(atMs);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** The pieces of a history row for the timeline: what happened, the old and new slot. */
export function historyParts(entry: PlanHistoryEntry): { head: string; before: string; after: string } {
  const head = entry.outcome === 'rejected_locked' ? `${actionLabel(entry.action)} (거절됨: 잠금 중)` : actionLabel(entry.action);
  const before = parseSnapshot(entry.beforeJson);
  const after = parseSnapshot(entry.afterJson);
  return {
    head,
    before: before === null ? '' : describeSnapshot(before),
    after: after === null ? '' : describeSnapshot(after),
  };
}
