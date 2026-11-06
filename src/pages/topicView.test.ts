import { describe, expect, it } from 'vitest';
import { excerpt, noticeText, reasonLabel, sourceLabel } from './topicView';

describe('topic view helpers', () => {
  it('explains every missing-topic case', () => {
    expect(noticeText('no_wiki')).toContain('위키');
    expect(noticeText('no_pages')).toContain('자료');
    expect(noticeText('no_plan')).toContain('계획');
  });

  it('labels where the page came from and why', () => {
    expect(sourceLabel('assigned')).toContain('배정');
    expect(reasonLabel('hard')).toContain('어려운');
    expect(reasonLabel(null)).toBe('');
  });

  it('cuts a long page at a character boundary', () => {
    expect(excerpt('  짧은 글 ', 10)).toBe('짧은 글');
    expect(excerpt('가나다라마', 3)).toBe('가나다…');
  });
});
