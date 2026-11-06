// Edge Function `missed-plan` (Deno). All decisions live in ../_shared (pure, tested); this file
// only wires the real I/O and checks who is calling.
//
// Callers:
//   * pg_cron once a minute, with `Authorization: Bearer <MISSED_PLAN_TOKEN>`  -> detection run
//   * the desktop app, with the signed-in user's JWT and {"action":"test"}      -> test message
//
// Secrets (Supabase function secrets, never in the repo or the app):
//   MISSED_PLAN_DB_KEY    server credential used for claim_alert and the cross-user reads
//   MISSED_PLAN_TOKEN     shared token the cron job sends
//   TELEGRAM_BOT_TOKEN    the one shared Telegram bot's token
//   NOTIFY_MODE           "mock" records instead of sending (never set in production)
// SUPABASE_URL and SUPABASE_ANON_KEY are provided by the platform.

import { DEFAULT_NTFY_SERVER, makeSender, RecordingChannel, type ChannelConfig, type Sender } from '../_shared/channels.ts';
import { ALERT_WINDOW_MS, type OccurrenceInfo } from '../_shared/detect.ts';
import { runChannelTest, runMissedPlanCheck, type Io, type UserCandidate } from '../_shared/handler.ts';

declare const Deno: {
  env: { get(name: string): string | undefined };
  serve(handler: (request: Request) => Response | Promise<Response>): void;
};

const url = Deno.env.get('SUPABASE_URL') ?? '';
const anonKey = Deno.env.get('SUPABASE_ANON_KEY') ?? '';
const dbKey = Deno.env.get('MISSED_PLAN_DB_KEY') ?? '';
const cronToken = Deno.env.get('MISSED_PLAN_TOKEN') ?? '';

const sender: Sender =
  Deno.env.get('NOTIFY_MODE') === 'mock'
    ? new RecordingChannel()
    : makeSender((u, init) => fetch(u, init), Deno.env.get('TELEGRAM_BOT_TOKEN'));

interface ChannelRow {
  user_id: string;
  channel: 'none' | 'ntfy' | 'telegram' | 'phone_app';
  ntfy_topic: string | null;
  ntfy_server: string | null;
  telegram_chat_id: string | null;
  missed_grace_min: number;
  utc_offset_min: number;
}

function toChannel(row: ChannelRow): ChannelConfig | null {
  if ((row.channel === 'ntfy' || row.channel === 'phone_app') && row.ntfy_topic) {
    return { channel: 'ntfy', server: row.ntfy_server ?? DEFAULT_NTFY_SERVER, topic: row.ntfy_topic };
  }
  if (row.channel === 'telegram' && row.telegram_chat_id) return { channel: 'telegram', chatId: row.telegram_chat_id };
  return null;
}

async function rest<T>(path: string, key: string, bearer: string): Promise<T> {
  const response = await fetch(`${url}/rest/v1/${path}`, { headers: { apikey: key, Authorization: `Bearer ${bearer}` } });
  if (!response.ok) throw new Error(`rest_${response.status}`);
  return (await response.json()) as T;
}

const realIo: Io = {
  sender,
  async listCandidates(nowMs) {
    const channels = await rest<ChannelRow[]>('notify_channels?channel=neq.none&select=*', dbKey, dbKey);
    if (channels.length === 0) return [];
    const from = nowMs - ALERT_WINDOW_MS;
    const occurrences = await rest<{ user_id: string; occurrence_key: string; start_at_utc_ms: number }[]>(
      `plan_occurrences?status=eq.planned&start_at_utc_ms=gt.${from}&start_at_utc_ms=lte.${nowMs}&select=user_id,occurrence_key,start_at_utc_ms`,
      dbKey,
      dbKey,
    );
    const seen = await rest<{ user_id: string; last_seen_at: string }[]>('presence?select=user_id,last_seen_at', dbKey, dbKey);
    const lastSeen = new Map<string, number>();
    for (const row of seen) lastSeen.set(row.user_id, Math.max(lastSeen.get(row.user_id) ?? 0, Date.parse(row.last_seen_at)));
    const candidates: UserCandidate[] = [];
    for (const row of channels) {
      const channel = toChannel(row);
      if (!channel) continue;
      const mine: OccurrenceInfo[] = occurrences
        .filter((o) => o.user_id === row.user_id)
        .map((o) => ({ key: o.occurrence_key, startAtUtcMs: o.start_at_utc_ms, status: 'planned' as const }));
      if (mine.length === 0) continue;
      candidates.push({
        userId: row.user_id,
        channel,
        graceMin: row.missed_grace_min,
        utcOffsetMin: row.utc_offset_min,
        lastSeenMs: lastSeen.get(row.user_id) ?? null,
        occurrences: mine,
      });
    }
    return candidates;
  },
  async claim(userId, occurrenceKey) {
    const response = await fetch(`${url}/rest/v1/rpc/claim_alert`, {
      method: 'POST',
      headers: { apikey: dbKey, Authorization: `Bearer ${dbKey}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ p_user_id: userId, p_occurrence_key: occurrenceKey, p_kind: 'missed_plan' }),
    });
    if (!response.ok) throw new Error(`claim_${response.status}`);
    return (await response.json()) === 'claimed' ? 'claimed' : 'duplicate';
  },
};

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
}

function sameSecret(a: string, b: string): boolean {
  if (a.length === 0 || a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i += 1) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

async function userTest(bearer: string): Promise<Response> {
  const who = await fetch(`${url}/auth/v1/user`, { headers: { apikey: anonKey, Authorization: `Bearer ${bearer}` } });
  if (!who.ok) return json({ ok: false, code: 'unauthorized' }, 401);
  const { id } = (await who.json()) as { id: string };
  const rows = await rest<ChannelRow[]>(`notify_channels?user_id=eq.${encodeURIComponent(id)}&select=*`, dbKey, dbKey);
  const result = await runChannelTest(sender, rows[0] ? toChannel(rows[0]) : null);
  return json(result, result.ok ? 200 : 502);
}

Deno.serve(async (request) => {
  const auth = request.headers.get('Authorization') ?? '';
  const bearer = auth.startsWith('Bearer ') ? auth.slice(7) : '';
  let body: { action?: string } = {};
  try {
    body = (await request.json()) as { action?: string };
  } catch {
    // The cron call has an empty object; anything unparsable is treated as the cron shape.
  }
  if (body.action === 'test') return userTest(bearer);
  if (!sameSecret(bearer, cronToken)) return json({ ok: false, code: 'unauthorized' }, 401);
  try {
    return json({ ok: true, ...(await runMissedPlanCheck(realIo, Date.now())) });
  } catch {
    return json({ ok: false, code: 'run_failed' }, 500);
  }
});
