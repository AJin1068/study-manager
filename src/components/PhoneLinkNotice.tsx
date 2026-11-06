import { useEffect, useState } from 'react';
import { commands, type PhoneLinkView } from '../api';
import { phoneLinkWarnings } from '../pages/phoneView';

const POLL_MS = 5_000;

/**
 * Warning under the timer while a paired phone is not under the lock (signal lost, blocking
 * service off, lock not delivered). Renders nothing when everything is fine or no phone is paired.
 */
export function PhoneLinkNotice() {
  const [view, setView] = useState<PhoneLinkView | null>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    let alive = true;
    const load = () => {
      commands
        .getPhoneLink()
        .then((next) => {
          if (alive) {
            setView(next);
            setNow(Date.now());
          }
        })
        .catch(() => {
          // The notice is advisory; a failed read leaves the last one on screen.
        });
    };
    load();
    const timer = window.setInterval(load, POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  const lines = view === null ? [] : phoneLinkWarnings(view, now);
  if (lines.length === 0) return null;
  return (
    <section className="card" role="alert" aria-labelledby="phone-link-title">
      <h2 id="phone-link-title">폰 잠금 경고</h2>
      {lines.map((line) => (
        <p key={line} className="error">
          {line}
        </p>
      ))}
    </section>
  );
}
