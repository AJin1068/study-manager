import { useCallback, useEffect, useState } from 'react';
import { commands, onPassChanged, type PassStatus, type ReleaseChallenge, type ReleaseKind } from '../api';
import type { TimerView } from '../hooks/useTimerState';
import { availableKinds, endsChallenge, kindLabel, outcomeMessage, passStatusText } from '../pages/releaseView';

/**
 * Release procedures: emergency pass, skip today, early release. Each is deliberately tedious: the
 * user types a long sentence the core issued. Pasting is blocked here as a convenience only; the
 * exact-match check, the daily limit, and the record are all in Rust.
 */
export function ReleasePanel({ timer }: { timer: TimerView }) {
  const [pass, setPass] = useState<PassStatus | null>(null);
  const [challenge, setChallenge] = useState<ReleaseChallenge | null>(null);
  const [typed, setTyped] = useState('');
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
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh, lockHeld]);

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
      setChallenge(await commands.beginRelease(kind));
      setTyped('');
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

  const kinds = availableKinds(timer.state, pass);
  return (
    <div className="release-box">
      {pass !== null && <p className="hint">{passStatusText(pass, now)}</p>}
      {challenge === null && kinds.length > 0 && (
        <div className="row">
          {kinds.map((kind) => (
            <button key={kind} type="button" onClick={() => void begin(kind)}>
              {kindLabel(kind)}
            </button>
          ))}
        </div>
      )}
      {challenge !== null && (
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
