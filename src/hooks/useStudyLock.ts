import { useCallback, useEffect, useState } from 'react';
import { commands, type StudyState, type UpdateOutcome } from '../api/commands';

export interface LockView {
  loaded: boolean;
  /** true or false once the state is known; null while unknown (not loaded, or the load failed). */
  studying: boolean | null;
  error: string | null;
  /** Change controls may be enabled: state known, not studying, no load error. */
  allowed: boolean;
}

/**
 * Pure mapping from the last load result to what the UI may show and enable.
 * Fail-closed: an unknown or failed state is `null`, never "not studying".
 */
export function lockView(state: StudyState | null, error: string | null): LockView {
  const studying = state === null ? null : state.studying;
  return {
    loaded: state !== null,
    studying,
    error,
    allowed: studying === false && error === null,
  };
}

/** Loads the study-lock state from the Rust core. The UI never decides the lock itself. */
export function useStudyLock(): LockView & { refresh: () => void } {
  const [state, setState] = useState<StudyState | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    commands
      .getStudyState()
      .then((value) => {
        setState(value);
        setError(null);
      })
      .catch((err: unknown) => {
        setState(null);
        setError(String(err));
      });
  }, []);

  useEffect(() => {
    let active = true;
    commands
      .getStudyState()
      .then((value) => {
        if (active) setState(value);
      })
      .catch((err: unknown) => {
        if (active) setError(String(err));
      });
    return () => {
      active = false;
    };
  }, []);

  return { ...lockView(state, error), refresh };
}

/** Human-readable result of a change attempt. Rejections are recorded in history by the core. */
export function describeOutcome(outcome: UpdateOutcome): string {
  return outcome.outcome === 'applied'
    ? '변경했습니다.'
    : '공부 중이라 변경하지 않았습니다. 시도는 기록에 남았습니다.';
}
