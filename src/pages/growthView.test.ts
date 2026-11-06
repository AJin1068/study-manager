import { describe, expect, it } from 'vitest';
import fixture from '../api/fixtures/growth-contract.json';
import type { GrowthStatus, WeeklyInsight } from '../api';
import { accuracyText, insightNote, levelText, reviewText, totalTimeText, wikiText } from './growthView';

const status = fixture.status as GrowthStatus;
const insight = fixture.insight as WeeklyInsight;

describe('growth view helpers', () => {
  it('writes the golden numbers', () => {
    expect(totalTimeText(status)).toBe('누적 1시간');
    expect(levelText(status)).toBe('Lv.2 씨앗 · 다음 레벨까지 15P');
    expect(wikiText(status)).toBe('위키 페이지 4개');
    expect(accuracyText(status)).toBe('퀴즈 정답률 70% (7/10)');
    expect(reviewText(status.nextReview)).toBe('수학 · 미분 (자주 틀리는 페이지)');
  });

  it('has a sentence for every empty state', () => {
    const empty: GrowthStatus = { ...status, totalStudyMs: 0, nextLevelAt: null, wikiPages: 0, quizAnswered: 0, quizCorrect: 0, quizAccuracyPct: null, nextReview: null };
    expect(totalTimeText(empty)).toContain('아직');
    expect(levelText(empty)).toContain('최고 레벨');
    expect(wikiText(empty)).toContain('없습니다');
    expect(accuracyText(empty)).toContain('아직');
    expect(reviewText(null)).toContain('없습니다');
  });

  it('notes the rule fallback only when the rule wrote the sentence', () => {
    expect(insightNote(insight)).toContain('기본 규칙');
    expect(insightNote({ ...insight, fallbackReason: 'bad_answer' })).toContain('답변');
    expect(insightNote({ ...insight, source: 'llm', fallbackReason: null })).toBeNull();
  });
});
