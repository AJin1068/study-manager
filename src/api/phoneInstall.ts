// Mirror of src-tauri/src/commands/phone_install.rs, checked against
// src/api/fixtures/phone-install-contract.json by both test suites.

import { invoke } from '@tauri-apps/api/core';
import type { UpdateOutcome } from './settings';

export interface BlockerUnsupported {
  kind: 'unsupported';
  reason: 'apple_screen_time_authorization_required';
}

export type BlockerStatus =
  | { kind: 'active' }
  | { kind: 'inactive' }
  | { kind: 'service_disabled' }
  | { kind: 'unreachable' }
  | BlockerUnsupported;

export interface PhoneInstallView {
  link: string;
  defaultLink: string;
  qrSvg: string | null;
  ios: BlockerStatus;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

export function isBlockerStatus(value: unknown): value is BlockerStatus {
  if (!isRecord(value)) return false;
  if (value.kind === 'active' || value.kind === 'inactive' || value.kind === 'service_disabled' || value.kind === 'unreachable') return true;
  return value.kind === 'unsupported' && value.reason === 'apple_screen_time_authorization_required';
}

export function isPhoneInstallView(value: unknown): value is PhoneInstallView {
  return (
    isRecord(value) &&
    typeof value.link === 'string' &&
    typeof value.defaultLink === 'string' &&
    (value.qrSvg === null || typeof value.qrSvg === 'string') &&
    isBlockerStatus(value.ios)
  );
}

export const phoneInstallCommands = {
  getPhoneInstall: () => invoke<PhoneInstallView>('get_phone_install'),
  updatePhoneApkLink: (link: string) => invoke<UpdateOutcome>('update_phone_apk_link', { link }),
};
