import { describe, expect, it } from 'vitest';
import fixture from './fixtures/points-contract.json';
import { GROWTH_STAGES, isLedgerEntry, isPointsStatus } from './points';

// The same fixture is serialized from Rust in `commands::points::tests::points_wire_shapes_match_the_shared_fixture`.
describe('points contract', () => {
  it('the shared shapes pass the production guards', () => {
    expect(isPointsStatus(fixture.status)).toBe(true);
    expect(isLedgerEntry(fixture.entry)).toBe(true);
    expect(fixture.stages).toEqual([...GROWTH_STAGES]);
  });

  it('accepts the top level (no next threshold) and a deduction', () => {
    expect(isPointsStatus({ ...fixture.status, nextLevelAt: null })).toBe(true);
    expect(isLedgerEntry({ ...fixture.entry, delta: -30 })).toBe(true);
  });

  it('rejects unknown stages, snake_case, and missing fields', () => {
    expect(isPointsStatus({ ...fixture.status, stage: 'tree' })).toBe(false);
    expect(isPointsStatus({ ...fixture.status, nextLevelAt: undefined })).toBe(false);
    expect(isPointsStatus({ ...fixture.status, stage_label: 'x', stageLabel: undefined })).toBe(false);
    expect(isLedgerEntry({ ...fixture.entry, ref_key: 'x', refKey: undefined })).toBe(false);
    expect(isLedgerEntry({ ...fixture.entry, delta: '20' })).toBe(false);
  });
});
