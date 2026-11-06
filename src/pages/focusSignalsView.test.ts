import { describe, expect, it } from 'vitest';
import { outcomeText, shortcutText } from './focusSignalsView';

describe('focusSignalsView', () => {
  it('words every outcome, including both ignore reasons', () => {
    expect(outcomeText({ status: 'recorded' })).toBe('기록했습니다.');
    expect(outcomeText({ status: 'acknowledged' })).toContain('집중');
    expect(outcomeText({ status: 'ignored', reason: 'tooSoon' })).toContain('이미');
    expect(outcomeText({ status: 'ignored', reason: 'notStudying' })).toContain('공부');
  });

  it('states a shortcut registration failure instead of hiding it', () => {
    expect(shortcutText(null)).toContain('확인');
    expect(shortcutText({ shortcut: 'Ctrl+D', shortcutRegistered: true, shortcutError: null })).toBe('단축키: Ctrl+D');
    const failed = shortcutText({ shortcut: 'Ctrl+D', shortcutRegistered: false, shortcutError: 'taken' });
    expect(failed).toContain('taken');
    expect(failed).toContain('버튼');
  });
});
