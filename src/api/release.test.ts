import { describe, expect, it } from 'vitest';
import fixture from './fixtures/release-contract.json';
import { isPassRecord, isPassStatus, isReleaseChallenge, isReleaseOutcome } from './release';

// The same fixture is serialized from Rust in `commands::release::tests::release_wire_shapes_match_the_shared_fixture`.
describe('release contract', () => {
  it('the shared shapes pass the production guards', () => {
    expect(isReleaseChallenge(fixture.challenge)).toBe(true);
    expect(fixture.outcomes).toHaveLength(7);
    expect(fixture.outcomes.every((entry: unknown) => isReleaseOutcome(entry))).toBe(true);
    expect(isPassStatus(fixture.passStatus)).toBe(true);
    expect(isPassRecord(fixture.record)).toBe(true);
  });

  it('rejects unknown outcomes, snake_case kinds, and missing nullable fields', () => {
    expect(isReleaseOutcome({ outcome: 'aborted' })).toBe(false);
    expect(isReleaseOutcome({ outcome: 'mismatch' })).toBe(false);
    expect(isReleaseOutcome({ outcome: 'released', kind: 'skip_today', expiresAtMs: null })).toBe(false);
    expect(isReleaseOutcome({ outcome: 'released', kind: 'skipToday' })).toBe(false);
    expect(isReleaseOutcome({ outcome: 'alreadyActive', expiresAtMs: null })).toBe(false);
    expect(isReleaseChallenge({ ...fixture.challenge, kind: 'abort' })).toBe(false);
    expect(isPassStatus({ ...fixture.passStatus, activeUntilMs: undefined })).toBe(false);
    expect(isPassRecord({ ...fixture.record, kind: 'emergencyPass' })).toBe(false);
    expect(isPassRecord({ ...fixture.record, penaltyPoints: undefined })).toBe(false);
  });
});
