import type { QuizContext, QuizItemResult } from '../api';

const TITLES: Record<QuizContext, string> = {
  distraction: '잠깐, 복습 문제 하나',
  release: '조기 해제 문제',
  session_end: '오늘 공부 마무리 퀴즈',
  checkin: '집중 확인 문제',
  review: '복습 문제',
};

export function cardTitle(context: QuizContext): string {
  return TITLES[context];
}

/** After answering a review card the right answer is shown. */
export function resultLine(result: QuizItemResult | undefined): string {
  if (result === undefined) return '';
  return result.correct ? '정답입니다.' : `정답: ${result.answer}`;
}
