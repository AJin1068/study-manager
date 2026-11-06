import { describe, expect, it } from 'vitest';
import fixture from './fixtures/enforce-contract.json';
import { isBlockAttempt, isBlockEntry, isBlockEvent, isEnforcementStatus, isRunningApp } from './enforce';

// The same fixture is serialized from Rust in `commands::enforce::tests::enforce_wire_shapes_match_the_shared_fixture`.
describe('enforcement contract', () => {
  it('the shared shapes pass the production guards', () => {
    expect(isBlockEntry(fixture.entry)).toBe(true);
    expect(isEnforcementStatus(fixture.status)).toBe(true);
    expect(isRunningApp(fixture.runningApp)).toBe(true);
    expect(isBlockAttempt(fixture.attempt)).toBe(true);
    expect(fixture.status.recentEvents.every((event: unknown) => isBlockEvent(event))).toBe(true);
  });

  it('rejects snake_case, unknown modes, and missing nullable fields', () => {
    expect(isEnforcementStatus({ ...fixture.status, mode: 'dry_run' })).toBe(false);
    expect(isEnforcementStatus({ ...fixture.status, recentEvents: [{ id: 1 }] })).toBe(false);
    expect(isBlockEvent({ ...fixture.status.recentEvents[0], detailCode: undefined })).toBe(false);
    expect(isBlockEntry({ ...fixture.entry, created_at_ms: 1, createdAtMs: undefined })).toBe(false);
    expect(isRunningApp({ id: 'x', displayName: 'X' })).toBe(false);
    expect(isBlockAttempt({ target_id: 'x', action: 'failed' })).toBe(false);
  });
});
