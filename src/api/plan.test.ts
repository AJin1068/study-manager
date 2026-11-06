import { describe, expect, it } from 'vitest';
import fixture from './fixtures/plan-contract.json';
import { isOccurrence, isPlanChange, isPlanHistoryEntry, isPlanStats } from './plan';

// The same fixture is serialized from Rust in `plan::tests::serialized_shapes_match_the_shared_fixture`.
describe('plan contract', () => {
  it('every PlanChange in the fixture passes the production guard', () => {
    expect(fixture.changes).toHaveLength(2);
    expect(fixture.changes.every((entry: unknown) => isPlanChange(entry))).toBe(true);
  });

  it('the fixture occurrence passes the production guard', () => {
    expect(isOccurrence(fixture.occurrence)).toBe(true);
  });

  it('rejects snake_case or missing fields the Rust side never emits', () => {
    expect(isOccurrence({ ...fixture.occurrence, source_date: 'x', sourceDate: undefined })).toBe(false);
    expect(isOccurrence({ ...fixture.occurrence, level: 'extreme' })).toBe(false);
    expect(isPlanChange({ outcome: { outcome: 'rejected_locked', key: 'plan' }, planId: null })).toBe(false);
    expect(isPlanChange({ outcome: { outcome: 'applied', key: 'plan' } })).toBe(false);
  });

  it('history entries use the DB action and reason strings', () => {
    expect(fixture.historyEntry.action).toBe('move');
    expect(fixture.historyEntry.reasonCode).toBe('postponed');
    expect(fixture.historyEntry.outcome).toBe('applied');
  });

  it('the fixture history entry and stats pass the production guards', () => {
    expect(isPlanHistoryEntry(fixture.historyEntry)).toBe(true);
    expect(isPlanStats(fixture.stats)).toBe(true);
  });

  it('history guard rejects unknown action strings and snake_case fields', () => {
    expect(isPlanHistoryEntry({ ...fixture.historyEntry, action: 'teleport' })).toBe(false);
    expect(isPlanHistoryEntry({ ...fixture.historyEntry, outcome: 'rejectedLocked' })).toBe(false);
    expect(isPlanHistoryEntry({ ...fixture.historyEntry, atMs: undefined, at_ms: 1 })).toBe(false);
    expect(isPlanStats({ ...fixture.stats, bySubject: [{ subject: 'x' }] })).toBe(false);
  });
});
