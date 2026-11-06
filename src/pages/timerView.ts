import type { NextPlan, RunSummary, SetSummary, TimerPhase, TimerState } from '../api';

export const PHASE_LABEL: Record<TimerPhase, string> = {
  idle: '공부 외 시간',
  study: '공부 중',
  break: '휴식 중',
  longBreak: '긴 휴식 중',
};

/** Keeps the fresher of two states: events and command results can arrive out of order. */
export function newerState(prev: TimerState | null, next: TimerState): TimerState {
  return prev !== null && prev.nowMs > next.nowMs ? prev : next;
}

/**
 * Milliseconds left in the current block. `offsetMs` is the core clock minus the local clock at
 * the moment the state arrived, so a wrong local clock does not skew the countdown.
 */
export function remainingMs(state: TimerState, offsetMs: number, localNowMs: number): number | null {
  if (state.plannedEndMs === null) return null;
  return Math.max(0, state.plannedEndMs - (localNowMs + offsetMs));
}

export function formatRemaining(ms: number): string {
  const total = Math.ceil(ms / 1000);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  const two = (n: number) => String(n).padStart(2, '0');
  return hours > 0 ? `${hours}:${two(minutes)}:${two(seconds)}` : `${two(minutes)}:${two(seconds)}`;
}

/** One-line description of the current block, for example `공부 중 · 2/3세트`. */
export function phaseText(state: TimerState): string {
  if (state.phase === 'idle') return state.lockHeld ? '계획 진행 중' : PHASE_LABEL.idle;
  return `${PHASE_LABEL[state.phase]} · ${state.setIndex}/${state.setsTotal}세트`;
}

function pad(n: number): string {
  return String(n).padStart(2, '0');
}

export function nextPlanText(next: NextPlan): string {
  const start = new Date(next.plannedStartMs);
  return `${next.name} (${next.subject}) · ${start.getMonth() + 1}/${start.getDate()} ${pad(start.getHours())}:${pad(start.getMinutes())} · ${next.sets}세트`;
}

/** `1시간 15분`, `25분`, `1분 미만`. */
export function formatStudied(ms: number): string {
  const minutes = Math.floor(Math.max(0, ms) / 60_000);
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (hours === 0 && rest === 0) return '1분 미만';
  if (hours === 0) return `${rest}분`;
  return rest === 0 ? `${hours}시간` : `${hours}시간 ${rest}분`;
}

export function attemptText(attemptCount: number): string {
  return `딴짓 시도 ${attemptCount}회`;
}

export function setSummaryText(summary: SetSummary): string {
  return `${summary.setIndex}세트 끝 · 이번 세트 ${formatStudied(summary.setStudiedMs)} · 오늘 ${formatStudied(summary.todayStudiedMs)} · ${attemptText(summary.attemptCount)}`;
}

export function runSummaryText(summary: RunSummary): string {
  const prefix = summary.status === 'completed' ? '계획 완료' : '계획 중단';
  return `${prefix} · 총 ${formatStudied(summary.runStudiedMs)} · 오늘 ${formatStudied(summary.todayStudiedMs)} · ${attemptText(summary.attemptCount)}`;
}
