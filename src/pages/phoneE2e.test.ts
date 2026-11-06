import { describe, expect, it } from 'vitest';
import { MAX_LEASE_MS, engageRequest, normalizeAllowList, type LockLeaseWire } from '../api/phoneBlocker';
import type { PairStatusView, PhoneLinkView } from '../api';
import { pairSummary, phoneLinkWarnings } from './phoneView';

// The UI-logic side of the stage 5 integration scenarios (the Rust side is
// src-tauri/src/phone/publisher_tests/e2e.rs): the same timeline, as the card and the bridge see it.

const MIN = 60_000;
const link = (over: Partial<PhoneLinkView> = {}): PhoneLinkView => ({
  paired: true,
  wantLocked: true,
  published: { seq: 1, state: 'locked', reason: null, atMs: 0 },
  publishFailing: false,
  lastError: null,
  openGaps: [],
  lastBeatMs: 0,
  serviceEnabled: true,
  ...over,
});
const status = (over: Partial<PairStatusView> = {}): PairStatusView => ({
  paired: true,
  pairedAtMs: 0,
  lastBeatMs: 0,
  serviceEnabled: true,
  online: true,
  ...over,
});
const lease = (over: Partial<LockLeaseWire> = {}): LockLeaseWire => ({
  run_id: '1:2',
  seq: 1,
  state: 'locked',
  release_cause: null,
  issued_at_ms: 0,
  valid_until_ms: 50 * MIN + 60_000,
  ...over,
});
const allow = (() => {
  const r = normalizeAllowList({ roles: ['emergency_call'], packages: [] });
  if (!r.ok) throw new Error('allow list');
  return r.value;
})();

describe('phone card through a study run', () => {
  it('is quiet while locked and healthy, warns on a lost lock signal, and goes quiet again', () => {
    expect(phoneLinkWarnings(link(), 10 * MIN)).toEqual([]);
    expect(pairSummary(status({ lastBeatMs: 9 * MIN }), 10 * MIN).tone).toBe('ok');

    // The server is unreachable while a lock is wanted: PC keeps working, the card says so.
    const out = phoneLinkWarnings(link({ publishFailing: true }), 10 * MIN);
    expect(out[0]).toContain('PC의 타이머와 차단은 그대로');
    // The phone went silent for 3 minutes.
    const silent = pairSummary(status({ lastBeatMs: 7 * MIN }), 10 * MIN);
    expect(silent.tone).toBe('warn');
    expect(silent.details.join(' ')).toContain('3분 전');

    // An emergency pass frees the phone: nothing is wanted, nothing to warn about.
    expect(phoneLinkWarnings(link({ wantLocked: false, publishFailing: false }), 12 * MIN)).toEqual([]);
  });

  it('shows the disabled service as a gap that is counted in minutes and not in the score', () => {
    const out = phoneLinkWarnings(link({ serviceEnabled: false, openGaps: [{ kind: 'service_disabled', sinceMs: 4 * MIN }] }), 10 * MIN);
    expect(out[0]).toContain('6분째');
    expect(out.at(-1)).toContain('점수에는 영향이 없습니다');
  });

  it('shows nothing for a revoked pairing, even in the middle of a run', () => {
    expect(phoneLinkWarnings(link({ paired: false, publishFailing: true, openGaps: [{ kind: 'unreachable', sinceMs: 0 }] }), 10 * MIN)).toEqual([]);
    expect(pairSummary({ paired: false, pairedAtMs: null, lastBeatMs: null, serviceEnabled: null, online: true }, 0).headline).toBe('연결된 폰 없음');
  });
});

describe('lease to engage mapping over the run', () => {
  it('engages a locked lease with its remaining time and refuses everything else', () => {
    const ok = engageRequest(lease(), 50 * MIN, allow);
    expect(ok).toEqual({ ok: true, value: { run_id: '1:2', seq: 1, remaining_ms: 50 * MIN, allow } });
    // An emergency pass is a free lease: it is never engaged.
    expect(engageRequest(lease({ seq: 2, state: 'free', release_cause: 'emergency_pass' }), 50 * MIN, allow)).toEqual({ ok: false, error: 'not_locked' });
    // The lease ran out on the device: nothing left to enforce.
    expect(engageRequest(lease(), 0, allow)).toEqual({ ok: false, error: 'bad_remaining' });
    expect(engageRequest(lease(), -1, allow)).toEqual({ ok: false, error: 'bad_remaining' });
    // Longer than the cap is never engaged, the cap itself is.
    expect(engageRequest(lease(), MAX_LEASE_MS + 1, allow)).toEqual({ ok: false, error: 'bad_remaining' });
    expect(engageRequest(lease(), MAX_LEASE_MS, allow).ok).toBe(true);
  });
});
