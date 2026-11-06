// Display helpers for the growth tab. The core computes every number; empty data is a normal
// state, never an error.

import type { GrowthStatus, NextReview, ReviewReason, WeeklyInsight } from '../api';
import { durationText } from './dashboardView';

const REASON_TEXT: Record<ReviewReason, string> = {
  empty: '내용이 거의 없는 페이지',
  low_accuracy: '자주 틀리는 페이지',
  stale: '오래 보지 않은 페이지',
  hard: '집중이 잘 되는 시간에 어려운 페이지',
  review: '복습할 페이지',
  chosen: '직접 고른 페이지',
};

export function totalTimeText(status: Pick<GrowthStatus, 'totalStudyMs'>): string {
  return status.totalStudyMs <= 0 ? '아직 인정된 공부 시간이 없습니다' : `누적 ${durationText(status.totalStudyMs)}`;
}

export function levelText(status: GrowthStatus): string {
  const base = `Lv.${status.level} ${status.stageLabel}`;
  if (status.nextLevelAt === null) return `${base} · 최고 레벨`;
  return `${base} · 다음 레벨까지 ${Math.max(0, status.nextLevelAt - status.earnedPoints)}P`;
}

export function wikiText(status: Pick<GrowthStatus, 'wikiPages'>): string {
  return status.wikiPages === 0 ? '위키 페이지가 없습니다' : `위키 페이지 ${status.wikiPages}개`;
}

export function accuracyText(status: Pick<GrowthStatus, 'quizAccuracyPct' | 'quizAnswered' | 'quizCorrect'>): string {
  if (status.quizAccuracyPct === null) return '아직 푼 퀴즈가 없습니다';
  return `퀴즈 정답률 ${status.quizAccuracyPct}% (${status.quizCorrect}/${status.quizAnswered})`;
}

export function reviewText(review: NextReview | null): string {
  if (review === null) return '복습할 위키 페이지가 없습니다';
  return `${review.subject} · ${review.pageName} (${REASON_TEXT[review.reason]})`;
}

/** The insight line and, when the local rule wrote it, a short note on why. */
export function insightNote(insight: WeeklyInsight): string | null {
  if (insight.source === 'llm' || insight.fallbackReason === null) return null;
  return insight.fallbackReason === 'bad_answer' ? '모델 답변을 쓸 수 없어 기본 문구를 보여 줍니다.' : '모델 없이 기본 규칙으로 만든 문구입니다.';
}
