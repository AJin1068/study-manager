import { useEffect, useState } from 'react';
import type { TimerView } from '../hooks/useTimerState';
import { ReleasePanel } from './ReleasePanel';
import { formatRemaining, nextPlanText, phaseText, remainingMs, runSummaryText, setSummaryText } from '../pages/timerView';

function useLocalNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return undefined;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [active]);
  return now;
}

const PHASE_PILL = { idle: 'p-plain', study: 'p-study', break: 'p-break', longBreak: 'p-break' } as const;

/**
 * The "지금" card: phase, big countdown, progress through the current segment, and the release
 * procedures (`ReleasePanel`). The countdown only reads the core's `plannedEndMs`; the core decides
 * every transition. `segmentMin` (the current segment's length from today's mode) only drives the bar.
 */
export function TimerPanel({ timer, segmentMin }: { timer: TimerView; segmentMin?: number | null }) {
  const { state } = timer;
  const now = useLocalNow(state !== null && state.plannedEndMs !== null);

  if (timer.error !== null) {
    return <p className="error">타이머 상태를 읽지 못했습니다: {timer.error}</p>;
  }
  if (state === null) return <p>불러오는 중</p>;

  const left = remainingMs(state, timer.offsetMs, now);
  const progress =
    left !== null && segmentMin != null && segmentMin > 0
      ? Math.min(100, Math.max(0, 100 - (left / (segmentMin * 60_000)) * 100))
      : null;
  const caption = state.phase === 'study' ? '휴식까지 남은 시간' : state.phase === 'idle' ? null : '공부 시작까지 남은 시간';

  return (
    <div className="timer-panel">
      <div className="row space-between">
        <h2>지금</h2>
        <span className={`pill ${PHASE_PILL[state.phase]}`}>{phaseText(state)}</span>
      </div>
      {left !== null ? (
        <>
          <div className="big">{formatRemaining(left)}</div>
          {caption !== null && <p className="hint">{caption}</p>}
          {progress !== null && (
            <div className="prog" role="img" aria-label={`현재 구간 ${Math.round(progress)}% 진행`}>
              <span style={{ width: `${progress}%` }} />
            </div>
          )}
          {state.next !== null && <p className="hint">다음 계획: {nextPlanText(state.next)}</p>}
        </>
      ) : (
        <p className="idle-line">{state.next !== null ? `다음 계획: ${nextPlanText(state.next)}` : '오늘 남은 계획이 없어요.'}</p>
      )}
      {timer.breakEndingSoon && <p className="hint">곧 공부가 시작됩니다. (1분 전)</p>}
      {timer.lastSet !== null && <p className="hint">{setSummaryText(timer.lastSet)}</p>}
      {timer.lastRun !== null && <p className="hint">{runSummaryText(timer.lastRun)}</p>}
      {timer.rechecked && <p className="hint">절전에서 깨어나 계획을 다시 확인했습니다.</p>}
      {timer.degraded && (
        <p className="warn" role="alert">
          자동 전환이 멈췄습니다. 아래 해제 절차는 계속 사용할 수 있습니다.
        </p>
      )}

      <ReleasePanel timer={timer} />
    </div>
  );
}
