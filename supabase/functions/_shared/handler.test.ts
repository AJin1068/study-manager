import { describe, expect, it } from 'vitest';
import { RecordingChannel, type ChannelConfig } from './channels.ts';
import { missedMessage, TEST_MESSAGE, type OccurrenceInfo } from './detect.ts';
import { runChannelTest, runMissedPlanCheck, type Io, type UserCandidate } from './handler.ts';

const MIN = 60_000;
const SEC = 1000;
const START = Date.UTC(2026, 5, 1, 11, 5, 0);
const CHANNEL: ChannelConfig = { channel: 'ntfy', server: 'https://ntfy.sh', topic: 'abcdefghijklmnopqrstuvwx' };

function world(over: Partial<UserCandidate> = {}, occurrences: OccurrenceInfo[] = [{ key: 's1:2026-06-01', startAtUtcMs: START, status: 'planned' }]) {
  const sender = new RecordingChannel();
  const claimed = new Set<string>();
  const user: UserCandidate = { userId: 'u1', channel: CHANNEL, graceMin: 3, utcOffsetMin: 540, lastSeenMs: null, occurrences, ...over };
  const io: Io = {
    listCandidates: async () => [user],
    claim: async (userId, key) => {
      const id = `${userId}|${key}`;
      if (claimed.has(id)) return 'duplicate';
      claimed.add(id);
      return 'claimed';
    },
    sender,
  };
  return { io, sender, user };
}

describe('handler end to end with injected I/O', () => {
  it('sends the time-only message once and a second tick sends nothing', async () => {
    const { io, sender } = world();
    const first = await runMissedPlanCheck(io, START + 3 * MIN + 5 * SEC);
    expect(first).toEqual({ considered: 1, sent: 1, duplicates: 0, failed: 0 });
    const second = await runMissedPlanCheck(io, START + 4 * MIN + 5 * SEC);
    expect(second).toEqual({ considered: 1, sent: 0, duplicates: 1, failed: 0 });
    expect(sender.sent).toHaveLength(1);
    expect(sender.sent[0].text).toBe(missedMessage(START, 540));
  });

  it('sends nothing for a skipped occurrence, a present PC, or before the grace', async () => {
    const skipped = world({}, [{ key: 'k', startAtUtcMs: START, status: 'skipped' }]);
    await runMissedPlanCheck(skipped.io, START + 4 * MIN);
    expect(skipped.sender.sent).toHaveLength(0);
    const present = world({ lastSeenMs: START + MIN });
    await runMissedPlanCheck(present.io, START + 4 * MIN);
    expect(present.sender.sent).toHaveLength(0);
    const early = world();
    await runMissedPlanCheck(early.io, START + 2 * MIN + 59 * SEC);
    expect(early.sender.sent).toHaveLength(0);
  });

  it('retries a failed send once and counts it failed after two failures', async () => {
    const flaky = world();
    flaky.sender.failNext = 1;
    expect(await runMissedPlanCheck(flaky.io, START + 4 * MIN)).toMatchObject({ sent: 1, failed: 0 });
    const down = world();
    down.sender.failNext = 2;
    expect(await runMissedPlanCheck(down.io, START + 4 * MIN)).toMatchObject({ sent: 0, failed: 1 });
  });

  it('no message ever contains a plan name, subject, or the channel secret', async () => {
    const occurrences = [{ key: 's1:2026-06-01', startAtUtcMs: START, status: 'planned' as const }];
    const { io, sender } = world({}, occurrences);
    await runMissedPlanCheck(io, START + 4 * MIN);
    for (const { text } of sender.sent) {
      expect(text).toMatch(/^\d\d:\d\d인데 아직 안 앉았어! 신호가 없어요\.$/);
      expect(text).not.toContain(CHANNEL.channel === 'ntfy' ? CHANNEL.topic : '');
    }
  });
});

describe('channel test', () => {
  it('sends the fixed test text to the channel', async () => {
    const sender = new RecordingChannel();
    expect(await runChannelTest(sender, CHANNEL)).toEqual({ ok: true, code: 'ok' });
    expect(sender.sent.map((s) => s.text)).toEqual([TEST_MESSAGE]);
  });
  it('reports a missing channel and a failed send with codes only', async () => {
    const sender = new RecordingChannel();
    expect(await runChannelTest(sender, null)).toEqual({ ok: false, code: 'not_configured' });
    sender.failNext = 1;
    expect(await runChannelTest(sender, CHANNEL)).toEqual({ ok: false, code: 'mock_failure' });
  });
});
