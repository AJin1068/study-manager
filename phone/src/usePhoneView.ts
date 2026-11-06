import { useCallback, useEffect, useState } from 'react';
import { forwardBlockerEvents, onView, phoneApi, type PhoneView } from './api';

export interface PhoneViewState {
  view: PhoneView | null;
  error: string | null;
  refresh: () => void;
}

/** The latest view: one read at mount, then whatever the Rust poll thread emits. */
export function usePhoneView(): PhoneViewState {
  const [view, setView] = useState<PhoneView | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    phoneApi
      .pollNow()
      .then((next) => {
        setView(next);
        setError(null);
      })
      .catch((err: unknown) => setError(String(err)));
  }, []);

  useEffect(() => {
    let cancelled = false;
    const cleanups: (() => void | Promise<void>)[] = [];
    phoneApi
      .view()
      .then((next) => !cancelled && setView(next))
      .catch((err: unknown) => !cancelled && setError(String(err)));
    onView((next) => setView(next))
      .then((off) => (cancelled ? off() : cleanups.push(off)))
      .catch((err: unknown) => !cancelled && setError(String(err)));
    forwardBlockerEvents()
      .then((off) => (cancelled ? void off() : cleanups.push(off)))
      .catch((err: unknown) => !cancelled && setError(String(err)));
    return () => {
      cancelled = true;
      cleanups.forEach((off) => void off());
    };
  }, []);

  return { view, error, refresh };
}
