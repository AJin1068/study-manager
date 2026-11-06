import { describe, expect, it } from 'vitest';
import fixture from './fixtures/update-outcome.json';
import generalSettingsFixture from './fixtures/general-settings.json';
import modeConfigFixture from './fixtures/mode-config.json';
import { isGeneralSettings, isModeConfig, isUpdateOutcome } from './index';

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

describe('GeneralSettings contract', () => {
  it('the shared fixture passes the production guard', () => {
    expect(isGeneralSettings(generalSettingsFixture)).toBe(true);
  });

  it('rejects a missing recheckPlanOnWake and snake_case keys', () => {
    expect(isGeneralSettings({ autostart: true, forceQuitAllowed: false })).toBe(false);
    expect(
      isGeneralSettings({ autostart: true, force_quit_allowed: false, recheck_plan_on_wake: true }),
    ).toBe(false);
  });
});

describe('ModeConfig contract', () => {
  it('every shared fixture entry passes the production guard', () => {
    expect(modeConfigFixture).toHaveLength(2);
    expect(modeConfigFixture.every((entry: unknown) => isModeConfig(entry))).toBe(true);
  });

  it('rejects unknown levels, string minutes, and undefined long-break fields', () => {
    expect(isModeConfig({ ...modeConfigFixture[0], level: 'extreme' })).toBe(false);
    expect(isModeConfig({ ...modeConfigFixture[0], studyMin: '25' })).toBe(false);
    expect(isModeConfig({ level: 'hard', studyMin: 90, breakMin: 10 })).toBe(false);
  });
});
