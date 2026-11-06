// The 5 minute promise as arithmetic: a plan starts, the PC is off, the cron fires once a minute at
// some phase, delivery takes up to 10 s. For every phase and grace the alert must go out exactly
// once and arrive in under 5 minutes.

import { describe, expect, it } from 'vitest';
import { RecordingChannel } from './channels.ts';
import { runMissedPlanCheck, type Io } from './handler.ts';

const SEC = 1000;
const MIN = 60 * SEC;
const DELIVERY_MS = 10 * SEC;
const START = Date.UTC(2026, 5, 1, 11, 5, 0);

async function simulate(graceMin: number, phaseMs: number) {
  const sender = new RecordingChannel();
  const claimed = new Set<string>();
  const sentAt: number[] = [];
  const io: Io = {
    listCandidates: async () => [
      {
        userId: 'u1',
        channel: { channel: 'ntfy', server: 'https://ntfy.sh', topic: 'abcdefghijklmnopqrstuvwx' },
        graceMin,
        utcOffsetMin: 540,
        lastSeenMs: null,
        occurrences: [{ key: 's1:2026-06-01', startAtUtcMs: START, status: 'planned' }],
      },
    ],
    claim: async (userId, key) => (claimed.has(`${userId}|${key}`) ? 'duplicate' : (claimed.add(`${userId}|${key}`), 'claimed')),
    sender,
  };
  // Ticks start two minutes before the plan and run past the 30 minute window.
  for (let tick = START - 2 * MIN + phaseMs; tick < START + 35 * MIN; tick += MIN) {
    const before = sender.sent.length;
    await runMissedPlanCheck(io, tick);
    if (sender.sent.length > before) sentAt.push(tick + DELIVERY_MS);
  }
  return { sends: sender.sent.length, sentAt };
}

describe('5 minute arithmetic', () => {
  for (const graceMin of [1, 2, 3]) {
    it(`grace ${graceMin} min: exactly one alert, under 5 minutes, at every cron phase`, async () => {
      let worst = 0;
      for (let phase = 0; phase < MIN; phase += 5 * SEC) {
        const { sends, sentAt } = await simulate(graceMin, phase);
        expect(sends).toBe(1);
        const delay = sentAt[0] - START;
        expect(delay).toBeGreaterThanOrEqual(graceMin * MIN);
        expect(delay).toBeLessThan(5 * MIN);
        worst = Math.max(worst, delay);
      }
      // Worst case is the grace + one tick interval (+ delivery): at most graceMin*60 + 70 s.
      expect(worst).toBeLessThanOrEqual(graceMin * MIN + MIN + DELIVERY_MS);
    });
  }

  it('the plan\'s own bound: grace 3 + cron 60 s + delivery 10 s = 4 min 10 s < 5 min', () => {
    expect(3 * MIN + MIN + DELIVERY_MS).toBe(4 * MIN + 10 * SEC);
    expect(3 * MIN + MIN + DELIVERY_MS).toBeLessThan(5 * MIN);
  });
});
