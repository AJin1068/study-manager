import { describe, expect, it } from 'vitest';
import fixture from '../api/fixtures/ics-contract.json';
import type { IcsApplyResult, IcsPreview } from '../api';
import { describeApplied, describePreview, describePreviewItem, describeSkipped, undoableBatch } from './calendarIoLogic';

const preview = fixture.preview as IcsPreview;
const applied = fixture.applyResult as IcsApplyResult;

describe('calendar import messages', () => {
  it('reports skipped events by reason, and nothing when none were skipped', () => {
    expect(describeSkipped(preview.skipped, preview.skippedTotal)).toBe('건너뜀 2건 (종일 일정 2)');
    expect(describeSkipped([], 0)).toBe('');
    expect(describePreview(preview)).toBe('가져올 계획 1개. 건너뜀 2건 (종일 일정 2)');
    expect(describePreview({ items: [], skipped: [], skippedTotal: 0 })).toBe('가져올 계획 0개');
  });

  it('describes repeating and single preview items', () => {
    expect(describePreviewItem(preview.items[0])).toBe('수학 (수학) · 매주 월·수 ~2026-12-31 20:00 · 3세트 · 제외 1회');
    expect(describePreviewItem({ ...preview.items[0], repeat: null, excludedDates: 0 })).toBe(
      '수학 (수학) · 2026-03-02 20:00 · 3세트',
    );
  });

  it('a locked import says it was rejected and offers no undo', () => {
    const locked: IcsApplyResult = {
      ...applied,
      change: { outcome: { outcome: 'rejectedLocked', key: 'plan' }, planId: null },
      batchId: null,
      imported: 0,
      skipped: [],
      skippedTotal: 0,
    };
    expect(describeApplied(locked)).toContain('거절');
    expect(undoableBatch(locked)).toBeNull();
  });

  it('an applied import shows the counts and offers undo for its batch only', () => {
    expect(describeApplied(applied)).toBe('계획 3개를 가져왔습니다. 건너뜀 1건 (다른 계획과 겹침 1)');
    expect(undoableBatch(applied)).toBe('ics-1767225600000-7');
    expect(undoableBatch({ ...applied, imported: 0, batchId: null })).toBeNull();
    expect(undoableBatch(null)).toBeNull();
  });
});
