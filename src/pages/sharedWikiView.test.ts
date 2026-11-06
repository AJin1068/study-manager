import { describe, expect, it } from 'vitest';
import fixture from '../api/fixtures/shared-wiki-contract.json';
import type { RoomPageView, SharedMerge } from '../api';
import { canMerge, mergeSummary, pageSummary, sharedCodeText, WITHDRAW_LIMIT } from './sharedWikiView';

const page = fixture.room.pages[0] as RoomPageView;
const merge = fixture.merge as SharedMerge;

describe('shared wiki view text', () => {
  it('words every code and never shows the raw code', () => {
    for (const code of ['ok', 'not_confirmed', 'not_member', 'too_large', 'not_consented', 'merge_not_enough', 'something_new']) {
      expect(sharedCodeText(code)).not.toContain('_');
    }
  });

  it('lists contributors and the people whose text is not merged', () => {
    const text = pageSummary(page);
    expect(text).toContain('앨리스, 밥');
    expect(text).toContain('합치기 미동의(목록만): 캐롤');
  });

  it('a merge needs two consented parts', () => {
    expect(canMerge(page)).toBe(true);
    expect(canMerge({ ...page, mergeableParts: 1 })).toBe(false);
  });

  it('a merge result credits the contributors and says whether the model was used', () => {
    expect(mergeSummary(merge)).toContain('AI가 합쳤어요');
    expect(mergeSummary(merge)).toContain('앨리스, 밥');
    expect(mergeSummary({ ...merge, usedLlm: false })).toContain('AI 없이');
    expect(mergeSummary({ ...merge, code: 'merge_not_enough' })).toBe(sharedCodeText('merge_not_enough'));
  });

  it('the withdraw notice states the limit', () => {
    expect(WITHDRAW_LIMIT).toContain('회수할 수 없어요');
  });
});
