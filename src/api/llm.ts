// Mirror of src-tauri/src/commands/llm.rs. The API key is saved through `saveSecret` (remote.ts)
// and never comes back; this file only carries the public switches and the usage numbers.

import { invoke } from '@tauri-apps/api/core';
import type { IntegrationMode } from './remote';
import type { UpdateOutcome } from './settings';

export type LlmSettingKey = 'enabled' | 'judgeEnabled' | 'model' | 'wikiDir' | 'dailyBudgetTokens';

export interface LlmConfig {
  enabled: boolean;
  judgeEnabled: boolean;
  model: string;
  /** Empty means the default folder. */
  wikiDir: string;
  dailyBudgetTokens: number;
}

export interface PurposeUsage {
  purpose: string;
  calls: number;
  tokens: number;
}

export interface LlmStatus {
  enabled: boolean;
  judgeEnabled: boolean;
  model: string;
  mode: IntegrationMode;
  dailyBudgetTokens: number;
  usedTodayTokens: number;
  byPurpose: PurposeUsage[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

export function isLlmConfig(value: unknown): value is LlmConfig {
  return (
    isRecord(value) &&
    typeof value.enabled === 'boolean' &&
    typeof value.judgeEnabled === 'boolean' &&
    typeof value.model === 'string' &&
    typeof value.wikiDir === 'string' &&
    typeof value.dailyBudgetTokens === 'number'
  );
}

export function isLlmStatus(value: unknown): value is LlmStatus {
  return (
    isRecord(value) &&
    typeof value.enabled === 'boolean' &&
    typeof value.judgeEnabled === 'boolean' &&
    typeof value.model === 'string' &&
    (value.mode === 'notConfigured' || value.mode === 'mock' || value.mode === 'live') &&
    typeof value.dailyBudgetTokens === 'number' &&
    typeof value.usedTodayTokens === 'number' &&
    Array.isArray(value.byPurpose) &&
    value.byPurpose.every(
      (row: unknown) =>
        isRecord(row) &&
        typeof row.purpose === 'string' &&
        typeof row.calls === 'number' &&
        typeof row.tokens === 'number',
    )
  );
}

export const llmCommands = {
  getLlmSettings: () => invoke<LlmConfig>('get_llm_settings'),
  updateLlmSetting: (key: LlmSettingKey, value: string | number | boolean) =>
    invoke<UpdateOutcome>('update_llm_setting', { key, value }),
  getLlmStatus: () => invoke<LlmStatus>('get_llm_status'),
};
