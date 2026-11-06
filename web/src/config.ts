// Runtime settings injected next to the bundle as config.json. Only the public project URL and
// the anon key belong here; the anon key is not a secret and every table is protected by RLS.

export type WebConfig = { mode: 'mock' } | { mode: 'live'; endpoint: string; anonKey: string };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

/** Accepts https endpoints (http only for localhost) and a non-empty anon key. */
export function parseConfig(raw: unknown): WebConfig | null {
  if (!isRecord(raw)) return null;
  if (raw.mode === 'mock') return { mode: 'mock' };
  if (raw.mode !== 'live' || typeof raw.endpoint !== 'string' || typeof raw.anonKey !== 'string') return null;
  let url: URL;
  try {
    url = new URL(raw.endpoint);
  } catch {
    return null;
  }
  const local = url.hostname === 'localhost' || url.hostname === '127.0.0.1';
  const secure = url.protocol === 'https:' || (url.protocol === 'http:' && local);
  if (!secure || url.username !== '' || url.password !== '' || url.search !== '' || url.hash !== '') return null;
  if (raw.anonKey.trim() === '') return null;
  return { mode: 'live', endpoint: url.origin + url.pathname.replace(/\/+$/, ''), anonKey: raw.anonKey.trim() };
}

export async function loadConfig(fetchFn: typeof fetch): Promise<WebConfig | null> {
  try {
    const response = await fetchFn('./config.json', { cache: 'no-store' });
    if (!response.ok) return null;
    return parseConfig(await response.json());
  } catch {
    return null;
  }
}
