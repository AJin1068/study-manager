import { describe, expect, it } from 'vitest';
import fixture from './fixtures/chat-contract.json';
import { isChatApplyResult, isChatDraft, isChatReply } from './chat';

// The same fixture is serialized from Rust in `commands::chat::tests::chat_wire_shapes_match_the_shared_fixture`.
describe('chat contract', () => {
  it('fixture values pass the production guards', () => {
    expect(isChatReply(fixture.reply)).toBe(true);
    expect(isChatApplyResult(fixture.applied)).toBe(true);
    expect(isChatReply({ ...fixture.reply, draft: null, source: 'local', fallbackReason: 'llm_disabled' })).toBe(true);
  });

  it('rejects snake_case, unknown sources, and unknown draft kinds', () => {
    expect(isChatReply({ ...fixture.reply, over_asked: false, overAsked: undefined })).toBe(false);
    expect(isChatReply({ ...fixture.reply, source: 'cloud' })).toBe(false);
    expect(isChatDraft({ id: 1, items: [{ kind: 'rename', label: 'x' }] })).toBe(false);
    expect(isChatApplyResult({ ...fixture.applied, changed: '1' })).toBe(false);
  });
});
