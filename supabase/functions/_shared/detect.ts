// Pure missed-plan detection. No I/O, no clock: the caller passes `nowMs`. The Edge Function and
// the tests share this one function, so the rule that is tested is the rule that runs.
//
// Rule (stage-2 plan, S2.6): alert for an occurrence when
//   now >= start + grace  and  now < start + 30 min  and  status = planned
//   and the PC sent no signal at or after the start.
// Out of scope, on purpose: a PC that was on before the start and went off in the middle (its last
// signal is after the start). A PC that is on with only the network down can raise a false alert,
// which is why the wording says "no signal" and not "you are not studying".

export const MIN_GRACE_MIN = 1;
export const MAX_GRACE_MIN = 3;
export const ALERT_WINDOW_MS = 30 * 60_000;
const MINUTE_MS = 60_000;

export type OccurrenceStatus = 'planned' | 'skipped' | 'canceled' | 'moved';

export interface OccurrenceInfo {
  key: string;
  startAtUtcMs: number;
  status: OccurrenceStatus;
}

export interface DetectInput {
  nowMs: number;
  graceMin: number;
  occurrences: readonly OccurrenceInfo[];
  /** Newest signal across the user's devices, or null if there never was one. */
  lastSeenMs: number | null;
}

/** The schema allows 1..3. Anything else (a bad row, a future edit) is held to the strictest legal value. */
export function normalizeGrace(graceMin: number): number {
  if (!Number.isInteger(graceMin) || graceMin < MIN_GRACE_MIN || graceMin > MAX_GRACE_MIN) {
    return MAX_GRACE_MIN;
  }
  return graceMin;
}

export function dueAlerts(input: DetectInput): OccurrenceInfo[] {
  const graceMs = normalizeGrace(input.graceMin) * MINUTE_MS;
  return input.occurrences.filter((o) => {
    if (o.status !== 'planned') return false;
    if (input.nowMs < o.startAtUtcMs + graceMs) return false;
    if (input.nowMs >= o.startAtUtcMs + ALERT_WINDOW_MS) return false;
    if (input.lastSeenMs !== null && input.lastSeenMs >= o.startAtUtcMs) return false;
    return true;
  });
}

/** "HH:MM" of the start on the user's own clock. Only the offset is known, never a place name. */
export function localClock(startAtUtcMs: number, utcOffsetMin: number): string {
  const shifted = new Date(startAtUtcMs + utcOffsetMin * MINUTE_MS);
  const hh = String(shifted.getUTCHours()).padStart(2, '0');
  const mm = String(shifted.getUTCMinutes()).padStart(2, '0');
  return `${hh}:${mm}`;
}

/** Lock-screen text: the time only. Never a subject or a plan name. */
export function missedMessage(startAtUtcMs: number, utcOffsetMin: number): string {
  return `${localClock(startAtUtcMs, utcOffsetMin)}인데 아직 안 앉았어! 신호가 없어요.`;
}

export const TEST_MESSAGE = '테스트 알림이에요. 잘 보이나요?';
