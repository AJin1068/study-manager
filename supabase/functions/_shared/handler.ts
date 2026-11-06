// The whole missed-plan flow with every outside effect injected: reading rows, claiming the
// alert, sending. Tests drive it with in-memory versions; index.ts wires the real ones.

import { ChannelError, type ChannelConfig, type Sender } from './channels.ts';
import { dueAlerts, missedMessage, normalizeGrace, TEST_MESSAGE, type OccurrenceInfo } from './detect.ts';

export interface UserCandidate {
  userId: string;
  channel: ChannelConfig;
  graceMin: number;
  utcOffsetMin: number;
  lastSeenMs: number | null;
  /** Planned occurrences whose start lies inside the alert window. */
  occurrences: readonly OccurrenceInfo[];
}

export interface Io {
  listCandidates(nowMs: number): Promise<UserCandidate[]>;
  /** `claim_alert`: 'claimed' exactly once per (user, occurrence). */
  claim(userId: string, occurrenceKey: string): Promise<'claimed' | 'duplicate'>;
  sender: Sender;
}

export interface RunReport {
  considered: number;
  sent: number;
  duplicates: number;
  failed: number;
}

const SEND_ATTEMPTS = 2;

/**
 * Claims first, sends second: a second tick can never double-send. The cost is that a send that
 * fails twice is not retried by later ticks; it is counted in `failed` and logged by the caller.
 */
export async function runMissedPlanCheck(io: Io, nowMs: number): Promise<RunReport> {
  const report: RunReport = { considered: 0, sent: 0, duplicates: 0, failed: 0 };
  const users = await io.listCandidates(nowMs);
  for (const user of users) {
    const due = dueAlerts({ nowMs, graceMin: normalizeGrace(user.graceMin), occurrences: user.occurrences, lastSeenMs: user.lastSeenMs });
    for (const occurrence of due) {
      report.considered += 1;
      if ((await io.claim(user.userId, occurrence.key)) === 'duplicate') {
        report.duplicates += 1;
        continue;
      }
      if (await sendWithRetry(io.sender, user.channel, missedMessage(occurrence.startAtUtcMs, user.utcOffsetMin))) {
        report.sent += 1;
      } else {
        report.failed += 1;
      }
    }
  }
  return report;
}

async function sendWithRetry(sender: Sender, channel: ChannelConfig, text: string): Promise<boolean> {
  for (let attempt = 0; attempt < SEND_ATTEMPTS; attempt += 1) {
    try {
      await sender.send(channel, text);
      return true;
    } catch {
      // try again; the code is not logged here because it may carry a URL
    }
  }
  return false;
}

/** "Send a test notification" for one signed-in user's own channel. */
export async function runChannelTest(sender: Sender, channel: ChannelConfig | null): Promise<{ ok: boolean; code: string }> {
  if (!channel) return { ok: false, code: 'not_configured' };
  try {
    await sender.send(channel, TEST_MESSAGE);
    return { ok: true, code: 'ok' };
  } catch (err) {
    return { ok: false, code: err instanceof ChannelError ? err.code : 'send_failed' };
  }
}
