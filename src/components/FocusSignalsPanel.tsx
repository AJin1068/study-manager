import { useCallback, useEffect, useState } from 'react';
import {
  commands,
  onFocusCheckin,
  onSelfReportRecorded,
  type CheckinPrompt,
  type SelfReportOutcome,
  type SelfReportStatus,
} from '../api';
import type { TimerView } from '../hooks/useTimerState';
import { outcomeText, shortcutText } from '../pages/focusSignalsView';

/** Mirror of `CHECKIN_ANSWER_WINDOW_MS` in focus/signals.rs; the core is what enforces it. */
const CHECKIN_ANSWER_WINDOW_MS = 120_000;

/**
 * "Just zoned out" button and the random check-in banner. Both only record during a locked study
 * block; the core decides and answers with an outcome, which is shown as-is.
 */
export function FocusSignalsPanel({ timer }: { timer: TimerView }) {
  const [status, setStatus] = useState<SelfReportStatus | null>(null);
  const [prompt, setPrompt] = useState<CheckinPrompt | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const studying = timer.state !== null && timer.state.phase === 'study' && timer.state.lockHeld;
  const segmentId = timer.state === null ? null : timer.state.segmentId;

  const fail = useCallback((err: unknown) => setNote(`실패: ${String(err)}`), []);
  const show = useCallback((outcome: SelfReportOutcome) => setNote(outcomeText(outcome)), []);

  useEffect(() => {
    commands.getSelfReportStatus().then(setStatus).catch(fail);
  }, [fail]);

  useEffect(() => {
    // A new block or a break: whatever was pending belongs to the core's answer now.
    commands.getPendingCheckin().then(setPrompt).catch(fail);
  }, [segmentId, studying, fail]);

  useEffect(() => {
    let active = true;
    const checkin = onFocusCheckin((next) => {
      if (active) setPrompt(next);
    });
    const recorded = onSelfReportRecorded((outcome) => {
      if (active) show(outcome);
    });
    return () => {
      active = false;
      for (const pending of [checkin, recorded]) pending.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, [show]);

  useEffect(() => {
    if (prompt === null) return undefined;
    const id = window.setTimeout(() => setPrompt(null), CHECKIN_ANSWER_WINDOW_MS);
    return () => window.clearTimeout(id);
  }, [prompt]);

  async function report() {
    try {
      show(await commands.reportDistraction());
    } catch (err) {
      fail(err);
    }
  }

  async function answer(focused: boolean) {
    try {
      show(await commands.answerCheckin(focused));
    } catch (err) {
      fail(err);
    } finally {
      setPrompt(null);
    }
  }

  return (
    <section className="card" aria-labelledby="focus-signals-title">
      <h2 id="focus-signals-title">집중 신호</h2>
      {prompt !== null && (
        <div role="alert">
          <p>방금 집중하고 있었나요?</p>
          <button type="button" onClick={() => void answer(true)}>
            네, 집중했어요
          </button>{' '}
          <button type="button" onClick={() => void answer(false)}>
            아니요, 딴생각했어요
          </button>
        </div>
      )}
      <button type="button" disabled={!studying} onClick={() => void report()}>
        방금 딴생각함
      </button>
      <p>{shortcutText(status)}</p>
      {!studying && <p>공부 시간에만 기록합니다.</p>}
      {note !== null && <p role="status">{note}</p>}
    </section>
  );
}
