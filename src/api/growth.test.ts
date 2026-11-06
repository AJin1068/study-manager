import { describe, expect, it } from 'vitest';
import fixture from './fixtures/growth-contract.json';
import { isGrowthStatus, isWeeklyInsight } from './growth';

// The same fixture is serialized from Rust in `commands::growth::tests::growth_wire_shapes_match_the_shared_fixture`.
describe('growth contract', () => {
  it('fixture values pass the production guards', () => {
    expect(isGrowthStatus(fixture.status)).toBe(true);
    expect(isWeeklyInsight(fixture.insight)).toBe(true);
  });

  it('accepts the empty state of a fresh database', () => {
    expect(isGrowthStatus({ ...fixture.status, quizAccuracyPct: null, nextReview: null, nextLevelAt: null, wikiPages: 0 })).toBe(true);
  });

  it('rejects snake_case, unknown reasons, and unknown insight sources', () => {
    expect(isGrowthStatus({ ...fixture.status, total_study_ms: 1, totalStudyMs: undefined })).toBe(false);
    expect(isGrowthStatus({ ...fixture.status, nextReview: { ...fixture.status.nextReview, reason: 'because' } })).toBe(false);
    expect(isWeeklyInsight({ ...fixture.insight, source: 'cloud' })).toBe(false);
  });
});
