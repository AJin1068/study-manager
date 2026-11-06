// Display helpers for the planning chat. The core decides everything (drafts, the lock, fallbacks);
// these only turn its answers into short Korean text.

import type { ChatApplyResult, ChatMessage, ChatReply, DraftKind } from '../api';

export const KIND_LABEL: Record<DraftKind, string> = { create: '추가', move: '이동', delete: '취소' };

const FALLBACK_TEXT: Record<string, string> = {
  llm_disabled: 'LLM이 꺼져 있습니다',
  llm_not_configured: 'LLM이 설정되지 않았습니다',
  llm_budget_exceeded: '오늘 LLM 사용량을 다 썼습니다',
};

/** Why the model did not answer, in words; unknown codes are shown as they are. */
export function fallbackText(code: string): string {
  return FALLBACK_TEXT[code] ?? `모델이 응답하지 못했습니다 (${code})`;
}

/** The one-line notice under a reply, or null when there is nothing to add. */
export function replyNotice(reply: ChatReply): string | null {
  if (reply.locked) return '공부 중에는 계획을 옮기거나 취소할 수 없습니다. 요청은 기록에 남았습니다.';
  if (reply.source === 'local' && reply.fallbackReason !== null) return `${fallbackText(reply.fallbackReason)}. 계획 초안은 만들 수 없습니다.`;
  if (reply.fallbackReason !== null) return `${fallbackText(reply.fallbackReason)}. 여기까지의 초안만 남았습니다.`;
  if (reply.overAsked) return '질문이 길어졌습니다. 날짜와 과목을 한 번에 알려 주세요.';
  return null;
}

export function applyNotice(result: ChatApplyResult): string {
  if (result.applied) return `계획 ${result.changed}건을 적용했습니다.`;
  if (result.locked) return '계획된 공부 진행 중이라 적용하지 않았습니다. 시도는 기록에 남았습니다.';
  return '적용하지 못했습니다.';
}

export function canSend(text: string, busy: boolean): boolean {
  return !busy && text.trim().length > 0 && text.trim().length <= 1000;
}

/** What the core is sent as history: only the spoken lines, never drafts or notices. */
export function toHistory(lines: ReadonlyArray<ChatMessage & { draftId?: number }>): ChatMessage[] {
  return lines.map(({ role, text }) => ({ role, text }));
}
