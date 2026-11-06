import { describe, expect, it } from 'vitest';
import { cardTitle, resultLine } from './quizView';

describe('quiz view helpers', () => {
  it('titles every context', () => {
    expect(cardTitle('session_end')).toContain('마무리');
    expect(cardTitle('distraction')).toContain('복습');
  });

  it('shows the right answer only for a wrong reply', () => {
    expect(resultLine({ itemId: 1, correct: true, answer: 'x' })).toBe('정답입니다.');
    expect(resultLine({ itemId: 1, correct: false, answer: 'x' })).toBe('정답: x');
    expect(resultLine(undefined)).toBe('');
  });
});
