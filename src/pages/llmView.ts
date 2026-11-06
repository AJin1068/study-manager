import type { LlmStatus } from '../api';

const ERROR_TEXT: Record<string, string> = {
  llm_disabled: 'LLM이 꺼져 있습니다.',
  llm_not_configured: 'API 키가 없어 쓸 수 없습니다.',
  llm_budget_exceeded: '오늘의 토큰 한도에 도달했습니다. 내일 다시 쓸 수 있고, 한도는 위에서 바꿀 수 있습니다.',
  llm_unauthorized: 'API 키가 거절되었습니다. 키를 확인하세요.',
  llm_rate_limited: '요청이 너무 많습니다. 잠시 뒤에 다시 시도하세요.',
  llm_overloaded: '서비스가 혼잡합니다. 잠시 뒤에 다시 시도하세요.',
  llm_timeout: '응답이 없습니다.',
  llm_host_denied: '허용되지 않은 주소라 연결하지 않았습니다.',
};

/** Plain-language reason for a stable LLM code; unknown codes are shown as codes. */
export function llmErrorText(code: string): string {
  return ERROR_TEXT[code] ?? `LLM을 쓸 수 없습니다 (${code}).`;
}

export function budgetSummary(status: LlmStatus): string {
  const pct = status.dailyBudgetTokens > 0 ? Math.min(100, Math.round((status.usedTodayTokens / status.dailyBudgetTokens) * 100)) : 0;
  return `오늘 ${status.usedTodayTokens.toLocaleString('ko-KR')} / ${status.dailyBudgetTokens.toLocaleString('ko-KR')} 토큰 (${pct}%)`;
}

const PURPOSE_LABELS: Record<string, string> = {
  wiki_ingest: '위키 정리',
  wiki_ask: '위키 묻기',
  probe: '연결 시험',
};

export function purposeLabel(purpose: string): string {
  return PURPOSE_LABELS[purpose] ?? purpose;
}
