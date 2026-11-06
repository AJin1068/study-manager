// Mirror of src-tauri/src/commands/context.rs: the opt-in "what was I studying" labels.

import { invoke } from '@tauri-apps/api/core';
import type { TitleAccess } from './focus';
import type { UpdateOutcome } from './settings';

export type ContextSettingKey = 'enabled' | 'windowTitles' | 'llmConsent';

export interface ContextStatus {
  enabled: boolean;
  windowTitles: boolean;
  llmConsent: boolean;
  retentionDays: number;
  rowCount: number;
  titleAccess: TitleAccess;
}

export function isContextStatus(value: unknown): value is ContextStatus {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    typeof r.enabled === 'boolean' &&
    typeof r.windowTitles === 'boolean' &&
    typeof r.llmConsent === 'boolean' &&
    typeof r.retentionDays === 'number' &&
    typeof r.rowCount === 'number' &&
    (r.titleAccess === 'granted' || r.titleAccess === 'denied' || r.titleAccess === 'unsupported')
  );
}

export const contextCommands = {
  getContextStatus: () => invoke<ContextStatus>('get_context_status'),
  updateContextSetting: (key: ContextSettingKey, value: boolean) =>
    invoke<UpdateOutcome>('update_context_setting', { key, value }),
  /** Deletes every stored label; resolves to the number of rows removed. */
  deleteStudyContext: () => invoke<number>('delete_study_context'),
};
