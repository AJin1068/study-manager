import { describe, expect, it } from 'vitest';
import fixture from './fixtures/update-outcome.json';
import { isUpdateOutcome } from './commands';

// The same fixture is serialized from Rust in `settings::tests::update_outcome_serialization_matches_shared_fixture`.
// This suite checks the production guard against it, so a rename on either side breaks a test.
describe('UpdateOutcome contract', () => {
  it('every fixture entry passes the production guard', () => {
    expect(fixture).toHaveLength(2);
    expect(fixture.every((entry: unknown) => isUpdateOutcome(entry))).toBe(true);
  });

  it('accepts exactly the applied and rejectedLocked shapes', () => {
    expect(isUpdateOutcome({ outcome: 'applied', key: 'autostart' })).toBe(true);
    expect(isUpdateOutcome({ outcome: 'rejectedLocked', key: 'studyMode' })).toBe(true);
  });

  it('rejects the snake_case outcome the Rust side never emits', () => {
    expect(isUpdateOutcome({ outcome: 'rejected_locked', key: 'autostart' })).toBe(false);
  });

  it('rejects a missing key and non-object values', () => {
    expect(isUpdateOutcome({ outcome: 'applied' })).toBe(false);
    expect(isUpdateOutcome(null)).toBe(false);
    expect(isUpdateOutcome('applied')).toBe(false);
  });
});
