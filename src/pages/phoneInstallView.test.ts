import { describe, expect, it } from 'vitest';
import contract from '../api/fixtures/phone-install-contract.json';
import { isBlockerStatus, isPhoneInstallView } from '../api';
import { ANDROID_INSTALL_STEPS, IOS_ALTERNATIVES, iosStatusText } from './phoneInstallView';

describe('phone install contract', () => {
  it('the shared shape passes the production guard', () => {
    expect(isPhoneInstallView(contract)).toBe(true);
    expect(contract.ios).toEqual({ kind: 'unsupported', reason: 'apple_screen_time_authorization_required' });
  });
  it('rejects snake_case, a wrong reason, and a missing link', () => {
    expect(isPhoneInstallView({ ...contract, default_link: 'x', defaultLink: undefined })).toBe(false);
    expect(isBlockerStatus({ kind: 'unsupported', reason: 'other' })).toBe(false);
    expect(isPhoneInstallView({ ...contract, link: undefined })).toBe(false);
  });
});

describe('install guidance', () => {
  it('explains the iPhone limit and offers alternatives', () => {
    expect(iosStatusText({ kind: 'unsupported', reason: 'apple_screen_time_authorization_required' })).toContain('스크린 타임');
    expect(IOS_ALTERNATIVES.length).toBeGreaterThanOrEqual(2);
  });
  it('lists the sideload steps', () => {
    expect(ANDROID_INSTALL_STEPS.some((step) => step.includes('출처를 알 수 없는 앱'))).toBe(true);
  });
});
