// Pure helpers for the calendar import/export panel (docs/PLAN.md feature 8, S2.8).

import type { IcsApplyResult, IcsPreview, IcsPreviewItem, IcsSkipCount, IcsSkipReason } from '../api';
import { weekdaysLabel } from './calendarLogic';

export const EXPORT_FILE_NAME = 'study-manager-plans.ics';

const SKIP_LABELS: Record<IcsSkipReason, string> = {
  allDay: '종일 일정',
  timezone: '시간대 지정 일정',
  unsupportedRule: '지원하지 않는 반복',
  invalid: '읽을 수 없는 일정',
  past: '이미 지난 일정',
  overlap: '다른 계획과 겹침',
};

export function skipReasonLabel(reason: IcsSkipReason): string {
  return SKIP_LABELS[reason];
}

/** `건너뜀 3건 (종일 일정 2, 다른 계획과 겹침 1)`; empty when nothing was skipped. */
export function describeSkipped(skipped: IcsSkipCount[], total: number): string {
  if (total === 0) return '';
  const parts = skipped.map((s) => `${skipReasonLabel(s.reason)} ${s.count}`);
  return `건너뜀 ${total}건 (${parts.join(', ')})`;
}

export function describePreviewItem(item: IcsPreviewItem): string {
  const when = item.repeat
    ? `매주 ${weekdaysLabel(item.repeat.weekdays)}${item.repeat.until ? ` ~${item.repeat.until}` : ''} ${item.startTime}`
    : `${item.startDate} ${item.startTime}`;
  const excluded = item.excludedDates > 0 ? ` · 제외 ${item.excludedDates}회` : '';
  return `${item.name} (${item.subject}) · ${when} · ${item.sets}세트${excluded}`;
}

export function describePreview(preview: IcsPreview): string {
  const head = `가져올 계획 ${preview.items.length}개`;
  const skipped = describeSkipped(preview.skipped, preview.skippedTotal);
  return skipped ? `${head}. ${skipped}` : head;
}

export function describeApplied(result: IcsApplyResult): string {
  if (result.change.outcome.outcome === 'rejectedLocked') {
    return '계획된 공부 진행 중이라 가져오기가 거절되었습니다. 진행이 끝난 뒤에 다시 시도하세요.';
  }
  const head = `계획 ${result.imported}개를 가져왔습니다`;
  const skipped = describeSkipped(result.skipped, result.skippedTotal);
  return skipped ? `${head}. ${skipped}` : `${head}.`;
}

/** Undo is offered only for an applied import that created something. */
export function undoableBatch(result: IcsApplyResult | null): string | null {
  if (result === null || result.imported === 0 || result.change.outcome.outcome !== 'applied') return null;
  return result.batchId;
}
