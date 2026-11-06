// Mirror of src-tauri/src/commands/browser_link.rs. Shapes are checked against
// src/api/fixtures/browser-contract.json, which the Rust side serializes in its own test.
// URLs and tab titles never cross this boundary: only domains, IDs, and fixed codes.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { UpdateOutcome } from './settings';
import type { BlockEntry } from './enforce';

export interface LinkStatus {
  /** The local listener is up. False means no extension can connect. */
  transportActive: boolean;
  transportError: string | null;
  connected: boolean;
  connections: number;
  lastConnectedAtMs: number | null;
  lastDisconnectedAtMs: number | null;
  /** Fixed reason code of the latest disconnect. */
  lastDisconnectReason: string | null;
}

export interface SiteRulesConfig {
  strict: boolean;
  /** `null` means strict mode uses the built-in set (`strictDefaultAllow`). */
  allow: string[] | null;
  youtubePlaylists: string[];
  youtubeChannels: string[];
}

export interface SiteRulesView {
  config: SiteRulesConfig;
  strictDefaultAllow: string[];
}

export interface PresetCategory {
  id: string;
  label: string;
  defaultOn: boolean;
  enabled: boolean;
  /** App IDs (macOS bundle ids; unverified against installed copies). */
  apps: string[];
  sites: string[];
}

export interface PresetStatus {
  /** The first-run choice has been made. */
  onboarded: boolean;
  version: number;
  categories: PresetCategory[];
}

export interface HostRegistration {
  supported: boolean;
  registered: boolean;
}

export interface Coverage {
  studyMs: number;
  unmonitoredMs: number;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === 'string');
}

function nullableNumber(value: unknown): boolean {
  return value === null || typeof value === 'number';
}

export function isLinkStatus(value: unknown): value is LinkStatus {
  if (!isRecord(value)) return false;
  return (
    typeof value.transportActive === 'boolean' &&
    (value.transportError === null || typeof value.transportError === 'string') &&
    typeof value.connected === 'boolean' &&
    typeof value.connections === 'number' &&
    nullableNumber(value.lastConnectedAtMs) &&
    nullableNumber(value.lastDisconnectedAtMs) &&
    (value.lastDisconnectReason === null || typeof value.lastDisconnectReason === 'string')
  );
}

export function isSiteRulesConfig(value: unknown): value is SiteRulesConfig {
  if (!isRecord(value)) return false;
  return (
    typeof value.strict === 'boolean' &&
    (value.allow === null || isStringArray(value.allow)) &&
    isStringArray(value.youtubePlaylists) &&
    isStringArray(value.youtubeChannels)
  );
}

export function isSiteRulesView(value: unknown): value is SiteRulesView {
  return isRecord(value) && isSiteRulesConfig(value.config) && isStringArray(value.strictDefaultAllow);
}

export function isPresetCategory(value: unknown): value is PresetCategory {
  if (!isRecord(value)) return false;
  return (
    typeof value.id === 'string' &&
    typeof value.label === 'string' &&
    typeof value.defaultOn === 'boolean' &&
    typeof value.enabled === 'boolean' &&
    isStringArray(value.apps) &&
    isStringArray(value.sites)
  );
}

export function isPresetStatus(value: unknown): value is PresetStatus {
  if (!isRecord(value)) return false;
  return (
    typeof value.onboarded === 'boolean' &&
    typeof value.version === 'number' &&
    Array.isArray(value.categories) &&
    value.categories.every(isPresetCategory)
  );
}

export function isHostRegistration(value: unknown): value is HostRegistration {
  return isRecord(value) && typeof value.supported === 'boolean' && typeof value.registered === 'boolean';
}

export function isCoverage(value: unknown): value is Coverage {
  return isRecord(value) && typeof value.studyMs === 'number' && typeof value.unmonitoredMs === 'number';
}

export const browserCommands = {
  getBrowserLinkStatus: () => invoke<LinkStatus>('get_browser_link_status'),
  getSiteRules: () => invoke<SiteRulesView>('get_site_rules'),
  updateSiteRules: (config: SiteRulesConfig) => invoke<UpdateOutcome>('update_site_rules', { config }),
  listSiteEntries: () => invoke<BlockEntry[]>('list_site_entries'),
  addSiteEntry: (pattern: string) => invoke<UpdateOutcome>('add_site_entry', { pattern }),
  getPresetStatus: () => invoke<PresetStatus>('get_preset_status'),
  /** `choice` maps category id to on/off. The first call is the first-run selection. */
  setPresetCategories: (choice: Record<string, boolean>) =>
    invoke<UpdateOutcome>('set_preset_categories', { choice }),
  getBrowserHostRegistration: () => invoke<HostRegistration>('get_browser_host_registration'),
  setBrowserHostRegistered: (registered: boolean) =>
    invoke<UpdateOutcome>('set_browser_host_registered', { registered }),
  getRunBrowserCoverage: (runId: number) => invoke<Coverage>('get_run_browser_coverage', { runId }),
};

/** Fired on every connect and disconnect of the extension link. */
export function onBrowserLinkChanged(handler: (status: LinkStatus) => void): Promise<UnlistenFn> {
  return listen<LinkStatus>('browser_link_changed', (event) => handler(event.payload));
}
