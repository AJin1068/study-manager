import type { PassStatus, ReleaseKind, ReleaseOutcome, TimerState } from '../api';

export const KIND_LABEL: Record<ReleaseKind, string> = {
  emergencyPass: '긴급 패스',
  skipToday: '오늘 건너뛰기',
  earlyRelease: '조기 해제',
};

export function kindLabel(kind: ReleaseKind): string {
  return KIND_LABEL[kind];
}

/**
 * Which releases to offer. All are costly, so none is offered when it could not take effect:
 * a pass and an early release need the lock; a skip also works before today's plan starts.
 * The core re-checks every one of these; this only avoids showing buttons that would be refused.
 */
export function availableKinds(state: TimerState | null, pass: PassStatus | null): ReleaseKind[] {
  if (state === null) return [];
  const kinds: ReleaseKind[] = [];
  if (state.lockHeld && pass !== null && pass.remainingToday > 0 && pass.activeUntilMs === null) {
    kinds.push('emergencyPass');
  }
  if (state.lockHeld || state.next !== null) kinds.push('skipToday');
  if (state.lockHeld) kinds.push('earlyRelease');
  return kinds;
}

function clock(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

function releasedMessage(kind: ReleaseKind, expiresAtMs: number | null): string {
  switch (kind) {
    case 'emergencyPass':
      return expiresAtMs === null
        ? '긴급 패스를 사용했습니다.'
        : `긴급 패스를 사용했습니다. ${clock(expiresAtMs)}까지 앱 차단이 멈춥니다. 잠금은 유지되며 사용 기록과 포인트 차감이 남습니다.`;
    case 'skipToday':
      return '오늘 남은 계획을 건너뛰었습니다. 건너뛰기는 기록에 남습니다.';
    case 'earlyRelease':
      return '공부를 조기 해제했습니다. 기록에 남습니다.';
  }
}

export function outcomeMessage(outcome: ReleaseOutcome): string {
  switch (outcome.outcome) {
    case 'released':
      return releasedMessage(outcome.kind, outcome.expiresAtMs);
    case 'mismatch':
      return `문장이 일치하지 않습니다. (${outcome.attempts}번째 시도)`;
    case 'noChallenge':
      return '입력 시간이 지났습니다. 다시 시작하세요.';
    case 'nothingToRelease':
      return '해제할 대상이 없습니다.';
    case 'limitReached':
      return `오늘의 긴급 패스(${outcome.limit}회)를 모두 사용했습니다.`;
    case 'alreadyActive':
      return `긴급 패스가 이미 ${clock(outcome.expiresAtMs)}까지 적용 중입니다.`;
  }
}

/** The settled outcomes end the challenge; a mismatch keeps it so the user can retype. */
export function endsChallenge(outcome: ReleaseOutcome): boolean {
  return outcome.outcome !== 'mismatch';
}

export function passStatusText(pass: PassStatus, nowMs: number): string {
  if (pass.activeUntilMs !== null && pass.activeUntilMs > nowMs) {
    return `긴급 패스 적용 중 (${clock(pass.activeUntilMs)}까지)`;
  }
  return `긴급 패스 오늘 ${pass.remainingToday}/${pass.dailyLimit}회 남음 · ${pass.durationMin}분 · -${pass.penaltyPoints}점`;
}
