// Notification channels. The request shapes are the ones in fixtures/notify/contract.json, which
// the desktop client (src-tauri/src/remote/notify.rs) is tested against too.

export type FetchLike = (url: string, init: { method: string; headers: Record<string, string>; body: string }) => Promise<{ ok: boolean; status: number }>;

export type ChannelConfig =
  | { channel: 'ntfy'; server: string; topic: string }
  | { channel: 'telegram'; chatId: string };

export const NTFY_TITLE = 'Study Manager';
export const DEFAULT_NTFY_SERVER = 'https://ntfy.sh';
const TELEGRAM_API = 'https://api.telegram.org';

const IPV4 = /^\d{1,3}(\.\d{1,3}){3}$/;

/**
 * The server posts to whatever address the user stored, so it must be a plain public https host:
 * no credentials, port, path, query, IP literal, or local/internal name. This keeps a stored
 * value from turning the function into a probe of the cloud provider's own network.
 */
export function isSafeNtfyServer(raw: string): boolean {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return false;
  }
  if (url.protocol !== 'https:') return false;
  if (url.username !== '' || url.password !== '' || url.port !== '') return false;
  if (url.pathname !== '/' || url.search !== '' || url.hash !== '') return false;
  const host = url.hostname.toLowerCase();
  if (host.startsWith('[') || IPV4.test(host)) return false;
  if (!host.includes('.')) return false;
  if (host === 'localhost' || host.endsWith('.localhost') || host.endsWith('.local') || host.endsWith('.internal')) return false;
  return true;
}

export interface Sender {
  send(config: ChannelConfig, text: string): Promise<void>;
}

export class ChannelError extends Error {
  constructor(public readonly code: string) {
    super(code);
  }
}

/** Real senders over an injected fetch. `telegramToken` is the Edge Function secret; it never reaches the app or the database. */
export function makeSender(fetchFn: FetchLike, telegramToken: string | undefined): Sender {
  return {
    async send(config, text) {
      if (config.channel === 'ntfy') {
        if (!isSafeNtfyServer(config.server)) throw new ChannelError('unsafe_server');
        const response = await fetchFn(`${config.server.replace(/\/+$/, '')}/${config.topic}`, {
          method: 'POST',
          headers: { Title: NTFY_TITLE, 'Content-Type': 'text/plain; charset=utf-8' },
          body: text,
        });
        if (!response.ok) throw new ChannelError(`http_${response.status}`);
        return;
      }
      if (!telegramToken) throw new ChannelError('telegram_not_configured');
      const response = await fetchFn(`${TELEGRAM_API}/bot${telegramToken}/sendMessage`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ chat_id: config.chatId, text }),
      });
      if (!response.ok) throw new ChannelError(`http_${response.status}`);
    },
  };
}

/** Mock channel (NOTIFY_MODE=mock and tests): keeps the messages, sends nothing. */
export class RecordingChannel implements Sender {
  readonly sent: { config: ChannelConfig; text: string }[] = [];
  failNext = 0;

  async send(config: ChannelConfig, text: string): Promise<void> {
    if (this.failNext > 0) {
      this.failNext -= 1;
      throw new ChannelError('mock_failure');
    }
    this.sent.push({ config, text });
  }
}
