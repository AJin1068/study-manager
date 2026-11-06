import { useEffect, useState } from 'react';
import { onCheerMessage, type CheerMessage } from '../api';

const SHOW_MS = 12_000;

/** Shows the latest cheer/scold message for a few seconds. It may name a wiki concept (in-app only). */
export function CheerToastHost() {
  const [message, setMessage] = useState<CheerMessage | null>(null);

  useEffect(() => {
    let active = true;
    const off = onCheerMessage((next) => {
      if (active) setMessage(next);
    });
    off.catch(() => undefined);
    return () => {
      active = false;
      off.then((stop) => stop()).catch(() => undefined);
    };
  }, []);

  useEffect(() => {
    if (message === null) return undefined;
    const timer = window.setTimeout(() => setMessage(null), SHOW_MS);
    return () => window.clearTimeout(timer);
  }, [message]);

  if (message === null) return null;
  return (
    <aside className="panel" role="status" aria-label="응원 메시지">
      <p>{message.text}</p>
    </aside>
  );
}
