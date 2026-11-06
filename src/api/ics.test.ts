import { describe, expect, it } from 'vitest';
import fixture from './fixtures/ics-contract.json';
import { isIcsApplyResult, isIcsExport, isIcsPreview } from './plan';

// The same fixture is serialized from Rust in `plan_import::tests::serialized_shapes_match_the_shared_fixture`.
describe('calendar import/export contract', () => {
  it('the fixture values pass the production guards', () => {
    expect(isIcsPreview(fixture.preview)).toBe(true);
    expect(isIcsApplyResult(fixture.applyResult)).toBe(true);
    expect(isIcsExport(fixture.export)).toBe(true);
  });

  it('rejects unknown skip reasons, snake_case fields, and a malformed change', () => {
    expect(isIcsPreview({ ...fixture.preview, skipped: [{ reason: 'weird', count: 1 }] })).toBe(false);
    expect(isIcsPreview({ ...fixture.preview, skippedTotal: undefined, skipped_total: 2 })).toBe(false);
    expect(isIcsPreview({ ...fixture.preview, items: [{ ...fixture.preview.items[0], repeat: { until: null } }] })).toBe(false);
    expect(isIcsApplyResult({ ...fixture.applyResult, change: { planId: 7 } })).toBe(false);
    expect(isIcsApplyResult({ ...fixture.applyResult, batchId: 5 })).toBe(false);
    expect(isIcsExport({ content: 'x' })).toBe(false);
  });
});
