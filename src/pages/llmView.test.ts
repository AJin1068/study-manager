import { describe, expect, it } from 'vitest';
import { budgetSummary, llmErrorText, purposeLabel } from './llmView';

describe('llmView', () => {
  it('explains known codes and shows unknown ones as codes', () => {
    expect(llmErrorText('llm_budget_exceeded')).toContain('한도');
    expect(llmErrorText('llm_weird')).toContain('llm_weird');
  });

  it('caps the budget percentage and survives a zero budget', () => {
    const base = { enabled: true, judgeEnabled: false, model: 'm', mode: 'mock' as const, byPurpose: [] };
    expect(budgetSummary({ ...base, dailyBudgetTokens: 100, usedTodayTokens: 250 })).toContain('(100%)');
    expect(budgetSummary({ ...base, dailyBudgetTokens: 0, usedTodayTokens: 5 })).toContain('(0%)');
  });

  it('falls back to the raw purpose', () => {
    expect(purposeLabel('wiki_ask')).toBe('위키 묻기');
    expect(purposeLabel('tab_judge')).toBe('tab_judge');
  });
});
