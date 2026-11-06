import { useEffect, useState } from 'react';
import { commands, onQuizCard, onRunSummary, type QuizCard, type QuizCardResult } from '../api';
import { cardTitle, resultLine } from '../pages/quizView';

/**
 * Shows review questions pushed by the core: after a blocked app or site is opened (the block
 * stays), at a check-in, and once a study run has ended. Answers are graded in Rust; this only
 * collects them. Nothing here lifts a block or ends a run.
 */
export function QuizCardHost() {
  const [card, setCard] = useState<QuizCard | null>(null);
  const [answers, setAnswers] = useState<string[]>([]);
  const [result, setResult] = useState<QuizCardResult | null>(null);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    const show = (next: QuizCard | null) => {
      if (!active || next === null) return;
      setCard(next);
      setAnswers(next.questions.map(() => ''));
      setResult(null);
      setNote(null);
    };
    const fail = (err: unknown) => {
      if (active) setNote(`실패: ${String(err)}`);
    };
    const pushed = onQuizCard(show);
    const ended = onRunSummary(() => {
      commands.getSessionEndQuiz().then(show).catch(fail);
    });
    pushed.catch(fail);
    ended.catch(fail);
    return () => {
      active = false;
      pushed.then((off) => off()).catch(() => undefined);
      ended.then((off) => off()).catch(() => undefined);
    };
  }, []);

  if (card === null) return null;

  async function submit(skip: boolean) {
    if (card === null) return;
    try {
      const graded = await commands.answerQuizCard(card.cardId, skip ? [] : answers, skip);
      if (graded === null || skip) {
        setCard(null);
      } else {
        setResult(graded);
      }
    } catch (err) {
      setNote(`실패: ${String(err)}`);
    }
  }

  return (
    <section className="panel" aria-label="복습 문제">
      <h2>{cardTitle(card.context)}</h2>
      {card.questions.map((question, index) => (
        <fieldset key={question.itemId} disabled={result !== null}>
          <legend>
            {index + 1}. {question.prompt}
          </legend>
          {question.kind === 'mcq' ? (
            question.choices.map((choice) => (
              <label key={choice} className="row">
                <input
                  type="radio"
                  name={`card-${card.cardId}-${question.itemId}`}
                  checked={answers[index] === choice}
                  onChange={() => setAnswers((prev) => prev.map((old, i) => (i === index ? choice : old)))}
                />
                {choice}
              </label>
            ))
          ) : (
            <input
              type="text"
              value={answers[index] ?? ''}
              aria-label={`${index + 1}번 빈칸 답`}
              onChange={(e) => setAnswers((prev) => prev.map((old, i) => (i === index ? e.target.value : old)))}
            />
          )}
          {result !== null && <p className="hint">{resultLine(result.results[index])}</p>}
        </fieldset>
      ))}
      {result !== null && result.bonusPoints > 0 && <p className="hint">보너스 +{result.bonusPoints}점</p>}
      <div className="row">
        {result === null ? (
          <>
            <button type="button" onClick={() => void submit(false)} disabled={answers.some((a) => a.trim().length === 0)}>
              제출
            </button>
            <button type="button" onClick={() => void submit(true)}>
              {card.skippable ? '건너뛰기(기록됨)' : '닫기'}
            </button>
          </>
        ) : (
          <button type="button" onClick={() => setCard(null)}>
            닫기
          </button>
        )}
      </div>
      {note !== null && <p className="error">{note}</p>}
    </section>
  );
}
