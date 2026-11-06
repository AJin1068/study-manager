import { describe, expect, it } from 'vitest';
import fixture from '../api/fixtures/plan-contract.json';
import type { Occurrence, PlanChange, PlanHistoryEntry } from '../api';
import {
  addDays,
  addMonths,
  buildCreateInput,
  buildDeleteInput,
  buildUpdateInput,
  derivedEnd,
  describeHistoryEntry,
  groupByDate,
  monthGrid,
  planChangeMessage,
  shiftAnchor,
  toggleWeekday,
  validateReason,
  visibleRange,
  weekDates,
  weekStart,
  weekdayIndex,
  weekdaysLabel,
} from './calendarLogic';

const occurrence = fixture.occurrence as Occurrence;
const history = fixture.historyEntry as PlanHistoryEntry;

describe('week and month math', () => {
  it('weeks start on Monday, matching the Rust bitmask', () => {
    expect(weekdayIndex('2026-03-09')).toBe(0);
    expect(weekdayIndex('2026-03-15')).toBe(6);
    expect(weekStart('2026-03-15')).toBe('2026-03-09');
    expect(weekDates('2026-03-11')).toEqual([
      '2026-03-09', '2026-03-10', '2026-03-11', '2026-03-12', '2026-03-13', '2026-03-14', '2026-03-15',
    ]);
  });

  it('addDays crosses month, year and leap day', () => {
    expect(addDays('2026-12-31', 1)).toBe('2027-01-01');
    expect(addDays('2028-02-28', 1)).toBe('2028-02-29');
    expect(addDays('2026-03-01', -1)).toBe('2026-02-28');
  });

  it('addMonths wraps years in both directions', () => {
    expect(addMonths('2026-12-15', 1)).toBe('2027-01-01');
    expect(addMonths('2026-01-31', -1)).toBe('2025-12-01');
  });

  it('month grid covers whole Monday-first weeks', () => {
    const grid = monthGrid('2026-02-10');
    // 2026-02-01 is a Sunday, so the first week starts in January.
    expect(grid).toHaveLength(5);
    expect(grid[0]?.[0]).toBe('2026-01-26');
    expect(grid[4]?.[6]).toBe('2026-03-01');
    expect(monthGrid('2026-03-10')[0]?.[0]).toBe('2026-02-23');
  });

  it('visible range and navigation follow the view', () => {
    expect(visibleRange('week', '2026-03-11')).toEqual({ from: '2026-03-09', to: '2026-03-15' });
    expect(visibleRange('month', '2026-03-11')).toEqual({ from: '2026-02-23', to: '2026-04-05' });
    expect(shiftAnchor('week', '2026-03-11', 1)).toBe('2026-03-18');
    expect(shiftAnchor('month', '2026-03-11', -1)).toBe('2026-02-01');
  });

  it('groups occurrences by date sorted by start time', () => {
    const early = { ...occurrence, startTime: '08:00' };
    const groups = groupByDate([occurrence, early]);
    expect(groups.get('2026-03-09')?.map((o) => o.startTime)).toEqual(['08:00', '20:00']);
  });
});

describe('derived end display', () => {
  it('derives the end from the duration', () => {
    expect(derivedEnd('20:00', 180)).toBe('20:00~23:00');
  });

  it('marks plans that cross midnight', () => {
    expect(derivedEnd('22:00', 190)).toBe('22:00~01:10 (+1일)');
  });
});

describe('weekday mask', () => {
  it('labels and toggles', () => {
    expect(weekdaysLabel(0b0000101)).toBe('월·수');
    expect(toggleWeekday(0b0000001, 2)).toBe(0b0000101);
    expect(toggleWeekday(0b0000101, 0)).toBe(0b0000100);
  });
});

describe('reason is required to submit an edit', () => {
  it('rejects no selection, and custom without text', () => {
    expect(validateReason({ code: null, text: '' }).ok).toBe(false);
    expect(validateReason({ code: 'custom', text: '  ' }).ok).toBe(false);
  });

  it('accepts a preset reason with optional text', () => {
    expect(validateReason({ code: 'sick', text: '' })).toEqual({
      ok: true,
      value: { reasonCode: 'sick', reasonText: null },
    });
    expect(validateReason({ code: 'custom', text: ' 시험 ' })).toEqual({
      ok: true,
      value: { reasonCode: 'custom', reasonText: '시험' },
    });
  });

  it('update and delete cannot be built without a reason', () => {
    const noReason = { code: null, text: '' } as const;
    const edit = buildUpdateInput(occurrence, {
      scope: 'occurrence', newDate: '2026-03-10', newTime: '20:00', sets: '3',
      weekdays: 0, repeatUntil: '', reason: noReason,
    });
    expect(edit.ok).toBe(false);
    expect(buildDeleteInput(occurrence, 'occurrence', noReason).ok).toBe(false);
  });
});

describe('building command inputs', () => {
  const reason = { code: 'postponed', text: '' } as const;

  it('sends only changed fields keyed by the original source date', () => {
    const built = buildUpdateInput(occurrence, {
      scope: 'occurrence', newDate: '2026-03-10', newTime: '20:00', sets: '3',
      weekdays: 0, repeatUntil: '', reason,
    });
    expect(built).toEqual({
      ok: true,
      value: {
        planId: 7, scope: 'occurrence', date: '2026-03-02', newDate: '2026-03-10',
        newTime: null, newSets: null, repeat: null, reasonCode: 'postponed', reasonText: null,
      },
    });
  });

  it('refuses an edit that changes nothing', () => {
    const built = buildUpdateInput(occurrence, {
      scope: 'occurrence', newDate: occurrence.date, newTime: occurrence.startTime,
      sets: String(occurrence.sets), weekdays: 0, repeatUntil: '', reason,
    });
    expect(built.ok).toBe(false);
  });

  it('series edits carry the repeat and ignore the date field', () => {
    const built = buildUpdateInput(occurrence, {
      scope: 'series', newDate: '2030-01-01', newTime: '21:00', sets: '3',
      weekdays: 0b0000001, repeatUntil: '', reason,
    });
    expect(built.ok && built.value.newDate).toBe(null);
    expect(built.ok && built.value.repeat).toEqual({ weekdays: 1, until: null });
  });

  it('create validates ranges and the repeat weekday of the start date', () => {
    const base = {
      name: '수학', subject: '수학', startDate: '2026-03-09', startTime: '20:00',
      sets: '3', weekdays: 0, repeatUntil: '',
    };
    expect(buildCreateInput(base).ok).toBe(true);
    expect(buildCreateInput({ ...base, sets: '0' }).ok).toBe(false);
    expect(buildCreateInput({ ...base, sets: '25' }).ok).toBe(false);
    expect(buildCreateInput({ ...base, sets: '1.5' }).ok).toBe(false);
    expect(buildCreateInput({ ...base, name: ' ' }).ok).toBe(false);
    expect(buildCreateInput({ ...base, startTime: '24:00' }).ok).toBe(false);
    expect(buildCreateInput({ ...base, startDate: '2026-02-30' }).ok).toBe(false);
    // 2026-03-09 is Monday (bit 0); Tuesday-only repeat excludes it.
    expect(buildCreateInput({ ...base, weekdays: 0b10 }).ok).toBe(false);
    expect(buildCreateInput({ ...base, weekdays: 0b11, repeatUntil: '2026-03-01' }).ok).toBe(false);
    expect(buildCreateInput({ ...base, weekdays: 0b11, repeatUntil: '2026-06-01' })).toMatchObject({
      ok: true,
      value: { repeat: { weekdays: 3, until: '2026-06-01' } },
    });
  });
});

describe('lock rejection notice', () => {
  it('shows a rejection, not a success, for rejectedLocked', () => {
    const rejected = fixture.changes[1] as PlanChange;
    expect(planChangeMessage(rejected).rejected).toBe(true);
    expect(planChangeMessage(rejected).text).toContain('기록에 남았습니다');
  });

  it('shows saved for applied', () => {
    const applied = fixture.changes[0] as PlanChange;
    expect(planChangeMessage(applied)).toEqual({ text: '계획을 저장했습니다.', rejected: false });
  });
});

describe('history display', () => {
  it('summarizes before and after from the Rust fixture', () => {
    expect(describeHistoryEntry(history)).toBe('옮기기: 2026-03-02 20:00 3세트 → 2026-03-09 20:00 3세트');
  });

  it('marks locked rejections and tolerates missing or malformed JSON', () => {
    const rejected: PlanHistoryEntry = { ...history, outcome: 'rejected_locked', beforeJson: null, afterJson: 'not json' };
    expect(describeHistoryEntry(rejected)).toBe('옮기기 (거절됨: 잠금 중)');
  });

  it('reads the plan snapshot shape used by create rows', () => {
    const created: PlanHistoryEntry = {
      ...history, action: 'create', beforeJson: null,
      afterJson: '{"name":"a","startDate":"2026-03-09","startTime":"20:00","sets":2}',
    };
    expect(describeHistoryEntry(created)).toBe('만들기: 2026-03-09 20:00 2세트');
  });
});
