import { useEffect, useState } from 'react';
import {
  commands,
  onBreakEndingSoon,
  onEngineDegraded,
  onPlanRechecked,
  onRunSummary,
  onSetSummary,
  onTimerChanged,
  type RunSummary,
  type SetSummary,
  type TimerState,
} from '../api';
import { newerState } from '../pages/timerView';

export interface TimerView {
  state: TimerState | null;
  /** Core clock minus local clock when `state` arrived. */
  offsetMs: number;
  error: string | null;
  /** The engine stopped making automatic transitions; the emergency release still works. */
  degraded: boolean;
  /** True once the system woke and the core re-derived the plan. */
  rechecked: boolean;
  /** The core announced that the current break ends in a minute. */
  breakEndingSoon: boolean;
  lastSet: SetSummary | null;
  lastRun: RunSummary | null;
}

interface Held {
  state: TimerState;
  offsetMs: number;
}

/** Snapshot from `get_timer_state` plus the core's `timer_changed`/`plan_rechecked`/`engine_degraded` events. */
export function useTimerState(): TimerView {
  const [held, setHeld] = useState<Held | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [degraded, setDegraded] = useState(false);
  const [rechecked, setRechecked] = useState(false);
  const [breakEndingSoon, setBreakEndingSoon] = useState(false);
  const [lastSet, setLastSet] = useState<SetSummary | null>(null);
  const [lastRun, setLastRun] = useState<RunSummary | null>(null);

  useEffect(() => {
    let active = true;
    const apply = (state: TimerState) => {
      if (!active) return;
      setHeld((prev) => {
        const kept = newerState(prev === null ? null : prev.state, state);
        return kept === state ? { state, offsetMs: state.nowMs - Date.now() } : prev;
      });
      setError(null);
    };
    const fail = (err: unknown) => {
      if (active) setError(String(err));
    };

    commands.getTimerState().then(apply).catch(fail);
    commands
      .getEngineHealth()
      .then((health) => {
        if (active) setDegraded(health.degraded);
      })
      .catch(fail);

    const unlisteners = [
      onTimerChanged((state) => {
        apply(state);
        if (active && state.phase !== 'break' && state.phase !== 'longBreak') setBreakEndingSoon(false);
      }),
      onBreakEndingSoon(() => {
        if (active) setBreakEndingSoon(true);
      }),
      onSetSummary((summary) => {
        if (active) setLastSet(summary);
      }),
      onRunSummary((summary) => {
        if (active) setLastRun(summary);
      }),
      onPlanRechecked((state) => {
        apply(state);
        if (active) setRechecked(true);
      }),
      onEngineDegraded((payload) => {
        if (active) setDegraded(payload.degraded);
      }),
    ];
    for (const pending of unlisteners) pending.catch(fail);

    return () => {
      active = false;
      for (const pending of unlisteners) {
        pending.then((unlisten) => unlisten()).catch(() => undefined);
      }
    };
  }, []);

  return {
    state: held === null ? null : held.state,
    offsetMs: held === null ? 0 : held.offsetMs,
    error,
    degraded,
    rechecked,
    breakEndingSoon,
    lastSet,
    lastRun,
  };
}
