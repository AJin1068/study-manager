import { describe, expect, it } from 'vitest';
import fixture from './fixtures/focus-contract.json';
import { isCheckinPrompt, isFocusSetup, isSelfReportOutcome, isSelfReportStatus, isStudyApp } from './focus';

// The same fixture is serialized from Rust in `commands::focus::tests::focus_wire_shapes_match_the_shared_fixture`.
describe('focus contract', () => {
  it('the shared shapes pass the production guards', () => {
    expect(isStudyApp(fixture.studyApp)).toBe(true);
    expect(isFocusSetup(fixture.setup)).toBe(true);
    expect(fixture.setup.needsStudyApps).toBe(fixture.setup.studyAppCount === 0);
  });

  it('the self-report shapes pass the production guards', () => {
    expect(isSelfReportStatus(fixture.selfReportStatus)).toBe(true);
    expect(isCheckinPrompt(fixture.checkinPrompt)).toBe(true);
    for (const outcome of Object.values(fixture.outcomes)) expect(isSelfReportOutcome(outcome)).toBe(true);
  });

  it('rejects unknown outcomes, snake_case, and a missing shortcut error', () => {
    expect(isSelfReportOutcome({ status: 'ignored' })).toBe(false);
    expect(isSelfReportOutcome({ status: 'ignored', reason: 'later' })).toBe(false);
    expect(isSelfReportOutcome({ status: 'done' })).toBe(false);
    expect(isSelfReportStatus({ ...fixture.selfReportStatus, shortcutError: undefined })).toBe(false);
    expect(isCheckinPrompt({ run_id: 1, segment_id: 2, asked_at_ms: 3 })).toBe(false);
  });

  it('rejects snake_case, unknown title access, and missing fields', () => {
    expect(isStudyApp({ ...fixture.studyApp, app_id: 'x', appId: undefined })).toBe(false);
    expect(isFocusSetup({ ...fixture.setup, titleAccess: 'maybe' })).toBe(false);
    expect(isFocusSetup({ ...fixture.setup, sensorsAvailable: undefined })).toBe(false);
  });
});
