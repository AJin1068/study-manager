import type { ModeChange, PlanChange } from '../api';
import { describeOutcome } from '../hooks/useStudyLock';

/** The toast content after a mode change; `undoBatchId` is set when the change moved plans. */
export interface ModeNotice {
  text: string;
  undoBatchId: string | null;
}

export function modeChangeNotice(change: ModeChange): ModeNotice {
  const base = describeOutcome(change);
  if (change.outcome !== 'applied' || change.shift === null) return { text: base, undoBatchId: null };
  if (change.shift.overflowsMidnight) {
    return {
      text: `${base} 새 길이로는 자정을 넘는 계획이 있어 계획 시각은 옮기지 않았습니다.`,
      undoBatchId: null,
    };
  }
  if (change.shift.movedCount === 0) return { text: base, undoBatchId: null };
  const by = change.shift.source === 'llm' ? 'AI가 ' : '';
  return {
    text: `${base} 오늘 아직 시작하지 않은 계획 ${change.shift.movedCount}개의 시작 시각을 ${by}옮겼습니다.`,
    undoBatchId: change.shift.batchId,
  };
}

export function undoNotice(change: PlanChange): ModeNotice {
  return {
    text:
      change.outcome.outcome === 'applied'
        ? '되돌렸습니다. 계획 시각과 모드를 원래대로 돌려놓았습니다.'
        : describeOutcome(change.outcome),
    undoBatchId: null,
  };
}

export function failureNotice(err: unknown): ModeNotice {
  return { text: `실패: ${String(err)}`, undoBatchId: null };
}
