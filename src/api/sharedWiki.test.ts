import { describe, expect, it } from 'vitest';
import fixture from './fixtures/shared-wiki-contract.json';
import { isSharedAction, isSharedMerge, isSharedQuizQuestion, isSharedRoomView, isSharedSync, isSharePreview } from './sharedWiki';

describe('shared wiki contract', () => {
  it('fixture values pass the production guards', () => {
    expect(isSharePreview(fixture.preview)).toBe(true);
    expect(isSharedAction(fixture.action)).toBe(true);
    expect(isSharedSync(fixture.sync)).toBe(true);
    expect(isSharedRoomView(fixture.room)).toBe(true);
    expect(isSharedMerge(fixture.merge)).toBe(true);
    expect(isSharedQuizQuestion(fixture.question)).toBe(true);
  });

  it('rejects snake_case fields and unknown question kinds', () => {
    expect(isSharedRoomView({ ...fixture.room, pages: [{ ...fixture.room.pages[0], has_merge: true, hasMerge: undefined }] })).toBe(false);
    expect(isSharedQuizQuestion({ ...fixture.question, kind: 'essay' })).toBe(false);
    expect(isSharedAction({ code: 'ok' })).toBe(false);
  });

  it('a quiz question carries no answer key', () => {
    expect(Object.keys(fixture.question).sort()).toEqual(['choices', 'itemId', 'kind', 'pageName', 'prompt']);
  });
});
