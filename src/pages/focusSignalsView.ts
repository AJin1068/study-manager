import type { SelfReportOutcome, SelfReportStatus } from '../api';

/** One-line feedback for a "just zoned out" press or a check-in answer. */
export function outcomeText(outcome: SelfReportOutcome): string {
  switch (outcome.status) {
    case 'recorded':
      return '기록했습니다.';
    case 'acknowledged':
      return '좋아요, 계속 집중해요.';
    case 'ignored':
      return outcome.reason === 'tooSoon'
        ? '방금 이미 기록했습니다.'
        : '공부 시간(잠금 중 공부 구간)에만 기록합니다.';
  }
}

/** Shortcut line; a registration failure is stated, never hidden. */
export function shortcutText(status: SelfReportStatus | null): string {
  if (status === null) return '단축키 상태를 확인하는 중';
  if (status.shortcutRegistered) return `단축키: ${status.shortcut}`;
  return `단축키를 쓸 수 없습니다(${status.shortcutError ?? '알 수 없는 이유'}). 버튼을 사용하세요.`;
}
