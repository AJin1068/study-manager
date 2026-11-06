import { describe, expect, it } from 'vitest';
import fixture from '../api/fixtures/mode-change.json';
import { isModeChange, type ModeChange } from '../api';
import { failureNotice, modeChangeNotice, undoNotice } from './modeShiftNotice';

const [moved, overflow, adjusted, fellBack, rejected] = fixture as ModeChange[];
if (!moved || !overflow || !adjusted || !fellBack || !rejected) throw new Error('fixture is incomplete');

describe('mode change contract', () => {
  it('every shared fixture entry passes the production guard', () => {
    expect(fixture).toHaveLength(5);
    expect(fixture.every((entry: unknown) => isModeChange(entry))).toBe(true);
  });

  it('rejects a missing shift, snake_case fields, and a malformed summary', () => {
    expect(isModeChange({ outcome: 'applied', key: 'studyMode' })).toBe(false);
    expect(
      isModeChange({ ...moved, shift: { batch_id: 'x', moved_count: 1, overflows_midnight: false, source: 'rule', fallback_code: null } }),
    ).toBe(false);
    expect(isModeChange({ ...moved, shift: { ...moved.shift, batchId: 3 } })).toBe(false);
    expect(isModeChange({ outcome: 'rejected_locked', key: 'studyMode', shift: null })).toBe(false);
    expect(isModeChange({ ...moved, shift: { ...moved.shift, source: 'other' } })).toBe(false);
    expect(isModeChange({ ...moved, shift: { batchId: null, movedCount: 0, overflowsMidnight: false } })).toBe(false);
  });
});

describe('modeChangeNotice', () => {
  it('offers undo with the batch id when plans moved', () => {
    const notice = modeChangeNotice(moved);
    expect(notice.undoBatchId).toBe('modeshift-3');
    expect(notice.text).toContain('1개');
  });

  it('says the AI moved the plans and still offers undo of that batch', () => {
    const notice = modeChangeNotice(adjusted);
    expect(notice.undoBatchId).toBe('llmadjust-9');
    expect(notice.text).toContain('AI가');
    expect(modeChangeNotice(fellBack).text).not.toContain('AI가');
  });

  it('warns about midnight overflow and offers no undo', () => {
    const notice = modeChangeNotice(overflow);
    expect(notice.undoBatchId).toBeNull();
    expect(notice.text).toContain('자정');
  });

  it('shows the plain result when nothing moved', () => {
    const notice = modeChangeNotice({
      outcome: 'applied',
      key: 'studyMode',
      shift: { batchId: null, movedCount: 0, overflowsMidnight: false, source: 'rule', fallbackCode: null },
    });
    expect(notice).toEqual({ text: '변경했습니다.', undoBatchId: null });
  });

  it('says the attempt was recorded when rejected, without undo', () => {
    const notice = modeChangeNotice(rejected);
    expect(notice.undoBatchId).toBeNull();
    expect(notice.text).toContain('기록에 남았습니다');
  });
});

describe('undoNotice and failureNotice', () => {
  it('reports an applied undo', () => {
    const notice = undoNotice({ outcome: { outcome: 'applied', key: 'plan' }, planId: 4 });
    expect(notice.text).toContain('되돌렸습니다');
    expect(notice.undoBatchId).toBeNull();
  });

  it('reports a locked undo as recorded', () => {
    const notice = undoNotice({ outcome: { outcome: 'rejectedLocked', key: 'plan' }, planId: 4 });
    expect(notice.text).toContain('기록에 남았습니다');
  });

  it('prefixes failures', () => {
    expect(failureNotice(new Error('boom')).text).toBe('실패: Error: boom');
  });
});
