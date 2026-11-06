import { describe, expect, it } from 'vitest';
import type { ChatReply } from '../api';
import { applyNotice, canSend, fallbackText, replyNotice, toHistory } from './chatView';

const reply: ChatReply = { text: 'x', source: 'llm', fallbackReason: null, draft: null, locked: false, overAsked: false };

describe('chat view helpers', () => {
  it('explains every reason the model did not answer', () => {
    expect(fallbackText('llm_disabled')).toContain('꺼져');
    expect(fallbackText('llm_budget_exceeded')).toContain('사용량');
    expect(fallbackText('llm_timeout')).toContain('llm_timeout');
  });

  it('puts the lock first, then the fallback, then the question limit', () => {
    expect(replyNotice(reply)).toBeNull();
    expect(replyNotice({ ...reply, locked: true, source: 'local', fallbackReason: 'llm_disabled' })).toContain('공부 중');
    expect(replyNotice({ ...reply, source: 'local', fallbackReason: 'llm_disabled' })).toContain('초안은 만들 수 없');
    expect(replyNotice({ ...reply, fallbackReason: 'llm_timeout' })).toContain('초안만 남았');
    expect(replyNotice({ ...reply, overAsked: true })).toContain('한 번에');
  });

  it('says what applying did', () => {
    expect(applyNotice({ applied: true, locked: false, changed: 2, batchId: 'chat-1' })).toBe('계획 2건을 적용했습니다.');
    expect(applyNotice({ applied: false, locked: true, changed: 0, batchId: null })).toContain('기록에 남았습니다');
  });

  it('sends only non-empty messages of a sane length, one at a time', () => {
    expect(canSend('  ', false)).toBe(false);
    expect(canSend('빈 시간', true)).toBe(false);
    expect(canSend('빈 시간', false)).toBe(true);
    expect(canSend('가'.repeat(1001), false)).toBe(false);
  });

  it('keeps drafts out of the history it sends', () => {
    expect(toHistory([{ role: 'user', text: 'a' }, { role: 'assistant', text: 'b', draftId: 3 }])).toEqual([
      { role: 'user', text: 'a' },
      { role: 'assistant', text: 'b' },
    ]);
  });
});
