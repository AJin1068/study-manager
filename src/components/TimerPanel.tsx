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

/**
 * Phase, countdown, next plan, and the release procedures (`ReleasePanel`). The countdown only
 * reads the core's `plannedEndMs`; the core decides every transition.
 */
export function TimerPanel({ timer }: { timer: TimerView }) {
  const { state } = timer;
  const now = useLocalNow(state !== null && state.plannedEndMs !== null);

  if (timer.error !== null) {
    return <p className="error">타이머 상태를 읽지 못했습니다: {timer.error}</p>;
  }
  if (state === null) return <p>불러오는 중</p>;

  const left = remainingMs(state, timer.offsetMs, now);

  return (
    <div className="timer-panel">
      <p>
        <strong>{phaseText(state)}</strong>
        {left !== null && <span className="countdown"> {formatRemaining(left)} 남음</span>}
      </p>
      {state.next !== null && <p className="hint">다음 계획: {nextPlanText(state.next)}</p>}
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
