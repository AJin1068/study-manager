import { describe, expect, it } from 'vitest';
import fixture from './fixtures/browser-contract.json';
import {
  isCoverage,
  isHostRegistration,
  isLinkStatus,
  isPresetStatus,
  isSiteRulesView,
} from './browser';

// The same fixture is serialized from Rust in `commands::browser_link::tests`.
describe('browser contract', () => {
  it('the shared shapes pass the production guards', () => {
    expect(isSiteRulesView(fixture.siteRules)).toBe(true);
    expect(isSiteRulesView(fixture.siteRulesDefaultAllow)).toBe(true);
    expect(isPresetStatus(fixture.presetStatus)).toBe(true);
    expect(isLinkStatus(fixture.linkStatus)).toBe(true);
    expect(isHostRegistration(fixture.hostRegistration)).toBe(true);
    expect(isCoverage(fixture.coverage)).toBe(true);
  });

  it('rejects snake_case, missing fields, and wrong types', () => {
    expect(isSiteRulesView({ ...fixture.siteRules, strict_default_allow: [], strictDefaultAllow: undefined })).toBe(false);
    expect(isSiteRulesView({ ...fixture.siteRules, config: { ...fixture.siteRules.config, allow: undefined } })).toBe(false);
    expect(isPresetStatus({ ...fixture.presetStatus, categories: [{ id: 'x' }] })).toBe(false);
    expect(isLinkStatus({ ...fixture.linkStatus, lastDisconnectReason: undefined })).toBe(false);
    expect(isCoverage({ studyMs: '1', unmonitoredMs: 0 })).toBe(false);
  });
});
