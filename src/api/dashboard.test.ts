import { describe, expect, it } from 'vitest';
import fixture from './fixtures/dashboard-contract.json';
import { isDayStat, isHeatmap, isTodayDashboard, isWeakness, isWeekDashboard } from './dashboard';

// The same fixture is serialized from Rust in `commands::dashboard::tests::dashboard_wire_shapes_match_the_shared_fixture`.
describe('dashboard contract', () => {
  it('the shared shapes pass the production guards', () => {
    expect(isTodayDashboard(fixture.today)).toBe(true);
    expect(isWeekDashboard(fixture.week)).toBe(true);
    expect(isWeakness(fixture.weakness)).toBe(true);
  });

  it('accepts the empty states the core returns for a fresh database', () => {
    expect(isTodayDashboard({ ...fixture.today, nextPlan: null, achievementPct: null, streakDays: 0 })).toBe(true);
    expect(isDayStat({ ...fixture.week.days[0], achievementPct: null, focusScore: null })).toBe(true);
    expect(isWeakness({ ...fixture.weakness, avgFirstAwayMs: null, deviationApps: [], postponedPlans: [] })).toBe(true);
  });

  it('rejects snake_case, missing fields, and a heatmap of the wrong size', () => {
    expect(isTodayDashboard({ ...fixture.today, planned_ms: 1, plannedMs: undefined })).toBe(false);
    expect(isTodayDashboard({ ...fixture.today, points: { level: 1 } })).toBe(false);
    expect(isWeekDashboard({ ...fixture.week, days: [{ ...fixture.week.days[0], achieved: 1 }] })).toBe(false);
    expect(isWeakness({ ...fixture.weakness, appOff: { sleepMs: 1 } })).toBe(false);
    expect(isHeatmap(fixture.weakness.heatmap.slice(1))).toBe(false);
    expect(isHeatmap(fixture.weakness.heatmap.map((row) => row.slice(1)))).toBe(false);
    expect(isHeatmap(fixture.weakness.heatmap)).toBe(true);
  });
});
