import { describe, expect, it } from 'vitest';
import { ALERT_WINDOW_MS, dueAlerts, localClock, missedMessage, normalizeGrace, type OccurrenceInfo } from './detect.ts';

const MIN = 60_000;
const START = Date.UTC(2026, 5, 1, 11, 5, 0); // 20:05 at +09:00
const occ = (over: Partial<OccurrenceInfo> = {}): OccurrenceInfo => ({ key: 's1:2026-06-01', startAtUtcMs: START, status: 'planned', ...over });
const keys = (nowMs: number, over: { graceMin?: number; lastSeenMs?: number | null; occurrences?: OccurrenceInfo[] } = {}) =>
  dueAlerts({ nowMs, graceMin: over.graceMin ?? 3, lastSeenMs: over.lastSeenMs ?? null, occurrences: over.occurrences ?? [occ()] }).map((o) => o.key);

describe('dueAlerts golden cases', () => {
  it('alerts when the PC never signalled and the grace has passed', () => {
    expect(keys(START + 3 * MIN)).toEqual(['s1:2026-06-01']);
  });
  it('alerts when the PC signalled only before the start (it was switched off)', () => {
    expect(keys(START + 4 * MIN, { lastSeenMs: START - 1 })).toEqual(['s1:2026-06-01']);
  });
  it('stays quiet when a signal arrived at or after the start', () => {
    expect(keys(START + 4 * MIN, { lastSeenMs: START })).toEqual([]);
    expect(keys(START + 4 * MIN, { lastSeenMs: START + 2 * MIN })).toEqual([]);
  });
  it('stays quiet for skipped, canceled and moved occurrences', () => {
    for (const status of ['skipped', 'canceled', 'moved'] as const) {
      expect(keys(START + 4 * MIN, { occurrences: [occ({ status })] })).toEqual([]);
    }
  });
  it('stays quiet once the 30 minute window is over', () => {
    expect(keys(START + ALERT_WINDOW_MS - 1)).toEqual(['s1:2026-06-01']);
    expect(keys(START + ALERT_WINDOW_MS)).toEqual([]);
    expect(keys(START + 2 * 60 * MIN)).toEqual([]);
  });
  it('handles two back-to-back plans independently', () => {
    const second = occ({ key: 's2:2026-06-01', startAtUtcMs: START + 10 * MIN });
    expect(keys(START + 5 * MIN, { occurrences: [occ(), second] })).toEqual(['s1:2026-06-01']);
    expect(keys(START + 13 * MIN, { occurrences: [occ(), second] })).toEqual(['s1:2026-06-01', 's2:2026-06-01']);
    // A signal at the second start silences only the second.
    expect(keys(START + 13 * MIN, { occurrences: [occ(), second], lastSeenMs: START + 10 * MIN })).toEqual([]);
  });
  it('is exact at the grace boundary, one second either side', () => {
    for (const graceMin of [1, 2, 3]) {
      const edge = START + graceMin * MIN;
      expect(keys(edge - 1000, { graceMin })).toEqual([]);
      expect(keys(edge - 1, { graceMin })).toEqual([]);
      expect(keys(edge, { graceMin })).toEqual(['s1:2026-06-01']);
      expect(keys(edge + 1000, { graceMin })).toEqual(['s1:2026-06-01']);
    }
  });
  it('never alerts before the start', () => {
    expect(keys(START - 1)).toEqual([]);
  });
});

describe('grace validation', () => {
  it('accepts 1..3 and holds anything else to 3', () => {
    expect([1, 2, 3].map(normalizeGrace)).toEqual([1, 2, 3]);
    for (const bad of [0, 4, -1, 2.5, Number.NaN, 180]) expect(normalizeGrace(bad)).toBe(3);
  });
  it('a grace of 4 behaves as 3, never later', () => {
    expect(keys(START + 3 * MIN, { graceMin: 4 })).toEqual(['s1:2026-06-01']);
  });
});

describe('wording', () => {
  it('shows the local start time and nothing about the subject', () => {
    expect(localClock(START, 540)).toBe('20:05');
    expect(localClock(START, -300)).toBe('06:05');
    expect(missedMessage(START, 540)).toBe('20:05인데 아직 안 앉았어! 신호가 없어요.');
  });
  it('the message is a function of the time alone', () => {
    // The only inputs are a timestamp and an offset: no plan or subject can reach the text.
    expect(missedMessage.length).toBe(2);
    expect(missedMessage(START, 540)).not.toMatch(/수학|영어|math|plan/i);
  });
});
