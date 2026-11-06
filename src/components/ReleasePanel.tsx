import { useCallback, useEffect, useState } from 'react';
import { commands, onPassChanged, type PassStatus, type ReleaseChallenge, type ReleaseKind, type ReleaseQuizStatus } from '../api';
import type { TimerView } from '../hooks/useTimerState';
import {
  availableKinds,
  earlyReleaseNotice,
  earlyReleaseStartable,
  endsChallenge,
  kindLabel,
  outcomeMessage,
  passStatusText,
  quizAnswersComplete,
} from '../pages/releaseView';

/**
 * Release procedures: emergency pass, skip today, early release. Each is deliberately tedious: the
 * user types a long sentence the core issued. Pasting is blocked here as a convenience only; the
 * exact-match check, the daily limit, and the record are all in Rust.
 *
 * Early release alone may ask wiki questions instead (all must be right; capped attempts and a
 * cooldown). The core sends the questions without answers and grades what is submitted; this
 * component never learns whether a single answer was right, only pass or fail.
 */
export function ReleasePanel({ timer }: { timer: TimerView }) {
  const [pass, setPass] = useState<PassStatus | null>(null);
  const [challenge, setChallenge] = useState<ReleaseChallenge | null>(null);
  const [typed, setTyped] = useState('');
  const [quiz, setQuiz] = useState<ReleaseQuizStatus | null>(null);
  const [answers, setAnswers] = useState<string[]>([]);
  const [note, setNote] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const lockHeld = timer.state === null ? null : timer.state.lockHeld;
  const passRunning = pass !== null && pass.activeUntilMs !== null;

  useEffect(() => {
    // Only a running pass has a clock to show.
    if (!passRunning) return undefined;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [passRunning]);

  const refresh = useCallback(() => {
    commands
      .getPassStatus()
      .then(setPass)
      .catch((err: unknown) => setNote(`실패: ${String(err)}`));
    commands
      .getReleaseQuizStatus()
      .then(setQuiz)
      .catch((err: unknown) => setNote(`실패: ${String(err)}`));
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh, lockHeld]);

  const cooldownMs = quiz !== null && quiz.mode === 'cooldown' ? quiz.cooldownRemainingMs : null;
  useEffect(() => {
    // The cooldown ends by itself; ask the core again when it should be over.
    if (cooldownMs === null) return undefined;
    const id = window.setTimeout(refresh, cooldownMs + 500);
    return () => window.clearTimeout(id);
  }, [cooldownMs, refresh]);

  useEffect(() => {
    let active = true;
    const pending = onPassChanged((status) => {
      if (active) setPass(status);
    });
    return () => {
      active = false;
      pending.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, []);

  async function begin(kind: ReleaseKind) {
    try {
      const issued = await commands.beginRelease(kind);
      setChallenge(issued);
      setTyped('');
      setAnswers(issued.questions.map(() => ''));
      setNote(null);
    } catch (err) {
      setNote(`실패: ${String(err)}`);
      refresh();
    }
  }

  async function submit() {
    if (challenge === null) return;
    try {
      const outcome = await commands.submitRelease(challenge.challengeId, typed);
      setNote(outcomeMessage(outcome));
      if (endsChallenge(outcome)) {
        setChallenge(null);
        refresh();
      }
      setTyped('');
    } catch (err) {
      setNote(`실패: ${String(err)}`);
    }
  }

  async function submitQuiz() {
    if (challenge === null) return;
    try {
      const outcome = await commands.submitReleaseQuiz(challenge.challengeId, answers);
      setNote(outcomeMessage(outcome));
      if (endsChallenge(outcome)) {
        setChallenge(null);
        refresh();
      }
      setAnswers([]);
    } catch (err) {
      setNote(`실패: ${String(err)}`);
    }
  }

  function setAnswer(index: number, value: string) {
    setAnswers((prev) => prev.map((old, i) => (i === index ? value : old)));
  }

  const kinds = availableKinds(timer.state, pass);
  return (
    <div className="release-box">
      {pass !== null && <p className="hint">{passStatusText(pass, now)}</p>}
      {challenge === null && kinds.length > 0 && (
        <div className="row">
          {kinds.map((kind) => (
            <button
              key={kind}
              type="button"
              disabled={kind === 'earlyRelease' && !earlyReleaseStartable(quiz)}
              onClick={() => void begin(kind)}
            >
              {kindLabel(kind)}
            </button>
          ))}
        </div>
      )}
      {challenge === null && kinds.includes('earlyRelease') && earlyReleaseNotice(quiz) !== null && (
        <p className="hint">{earlyReleaseNotice(quiz)}</p>
      )}
      {challenge !== null && challenge.proof === 'quiz' && (
        <div>
          <p>
            <strong>{kindLabel(challenge.kind)}</strong>: 위키 문제 {challenge.questions.length}개를 모두 맞히면 해제돼요.
            {challenge.attemptsLeft !== null && ` 오늘 남은 시도는 ${challenge.attemptsLeft}번이에요.`} 창을 닫아도 이 시도는 사용한 것으로 세요.
          </p>
          {challenge.questions.map((question, index) => (
            <fieldset key={question.itemId}>
              <legend>
                {index + 1}. {question.prompt}
              </legend>
              {question.kind === 'mcq' ? (
                question.choices.map((choice) => (
                  <label key={choice} className="row">
                    <input
                      type="radio"
                      name={`release-q-${question.itemId}`}
                      checked={answers[index] === choice}
                      onChange={() => setAnswer(index, choice)}
                    />
                    {choice}
                  </label>
                ))
              ) : (
                <input
                  type="text"
                  value={answers[index] ?? ''}
                  aria-label={`${index + 1}번 빈칸 답`}
                  onChange={(e) => setAnswer(index, e.target.value)}
                />
              )}
            </fieldset>
          ))}
          <div className="row">
            <button type="button" onClick={() => void submitQuiz()} disabled={!quizAnswersComplete(answers, challenge.questions.length)}>
              제출
            </button>
            <button type="button" onClick={() => setChallenge(null)}>
              취소
            </button>
          </div>
        </div>
      )}
      {challenge !== null && challenge.proof === 'typing' && (
        <div>
          <p>
            <strong>{kindLabel(challenge.kind)}</strong>: 아래 문장을 그대로 직접 입력하세요. 붙여넣기는 막혀 있습니다.
          </p>
          <blockquote>{challenge.sentence}</blockquote>
          <textarea
            value={typed}
            rows={3}
            aria-label="해제 문장 입력"
            onChange={(e) => setTyped(e.target.value)}
            onPaste={(e) => e.preventDefault()}
            onDrop={(e) => e.preventDefault()}
          />
          <div className="row">
            <button type="button" onClick={() => void submit()} disabled={typed.length === 0}>
              {kindLabel(challenge.kind)} 적용
            </button>
            <button type="button" onClick={() => setChallenge(null)}>
              취소
            </button>
          </div>
        </div>
      )}
      {note !== null && <p role="status">{note}</p>}
    </div>
  );
}
