import { describe, expect, it } from 'vitest';
import type { IngestView } from '../api';
import { askSummary, canResume, ingestSummary, skipReasonText } from './wikiView';

const base: IngestView = {
  previewId: 'p',
  subjectId: 1,
  status: 'ready',
  processedSteps: 1,
  totalSteps: 1,
  rejectedSteps: 0,
  usedFallback: false,
  stoppedBy: null,
  proposals: [],
  skipped: [],
};

describe('wikiView', () => {
  it('says when the local fallback made the draft', () => {
    expect(ingestSummary({ ...base, usedFallback: true })).toContain('LLM 없이');
  });

  it('a stopped import names the reason and can resume, a finished one cannot', () => {
    const stopped: IngestView = { ...base, status: 'stopped', stoppedBy: 'llm_budget_exceeded', processedSteps: 2, totalSteps: 5 };
    expect(ingestSummary(stopped)).toContain('한도');
    expect(canResume(stopped)).toBe(true);
    expect(canResume(base)).toBe(false);
    expect(canResume({ ...stopped, previewId: null })).toBe(false);
  });

  it('explains discarded and unavailable answers', () => {
    expect(askSummary({ status: 'ungrounded', matches: [] })).toContain('버렸');
    expect(askSummary({ status: 'unavailable', code: 'llm_disabled', matches: [] })).toContain('꺼져');
    expect(askSummary({ status: 'answer', askId: 'a', answer: 'x', citations: [], droppedCitations: 0 })).toBe('');
  });

  it('shows unknown skip reasons as is', () => {
    expect(skipReasonText('not_text')).toContain('텍스트');
    expect(skipReasonText('weird')).toBe('weird');
  });
});
