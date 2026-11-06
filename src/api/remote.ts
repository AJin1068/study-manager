// Mirror of src-tauri/src/commands/remote.rs. Types match the Rust serde output and are checked
// against src/api/fixtures/remote-contract.json by both test suites.
// Secret values go in through `saveSecret` only and are never returned.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { UpdateOutcome } from './settings';

export const INTEGRATION_PORTS = ['supabase', 'notify', 'llm', 'push'] as const;
export type IntegrationPort = (typeof INTEGRATION_PORTS)[number];

/** What the user chose. `live` is never chosen: it is derived from configuration. */
export type ModeSetting = 'auto' | 'mock';
export type IntegrationMode = 'notConfigured' | 'mock' | 'live';

export type Health = { kind: 'ok' } | { kind: 'degraded'; code: string } | { kind: 'unknown' };

export interface IntegrationState {
  port: IntegrationPort;
  setting: ModeSetting;
  mode: IntegrationMode;
  configured: boolean;
  health: Health;
  lastOkMs: number | null;
}

export interface SecretFlags {
  anthropicApiKey: boolean;
  supabaseRefreshToken: boolean;
  ntfyTopic: boolean;
}

export type SecretStoreKind = 'keyring' | 'inMemory' | 'sessionOnly';

export interface SecretStoreStatus {
  kind: SecretStoreKind;
  /** `secret_store_session_only` when the OS keychain was unusable: keys vanish on restart. */
  degradedCode: string | null;
  configured: SecretFlags;
}

export interface AuthStatus {
  signedIn: boolean;
  userId: string | null;
  /** `auth_expired` when the stored login stopped working and a new sign-in is needed. */
  degradedCode: string | null;
  cooldownUntilMs: number | null;
}

export interface IntegrationStatesView {
  ports: IntegrationState[];
  secretStore: SecretStoreStatus;
  auth: AuthStatus;
}

/** Secrets the user may enter by hand. The refresh token only comes from a sign-in. */
export type SecretName = 'anthropic_api_key' | 'ntfy_topic';
export type StoredSecretName = SecretName | 'supabase_refresh_token';

export type RemoteSettingKey =
  | 'supabaseEndpoint'
  | 'supabaseAnonKey'
  | 'notifyChannel'
  | 'notifyNtfyServer'
  | 'notifyTelegramChatId'
  | 'notifyMissedGraceMin';

export interface RemoteConfig {
  supabaseEndpoint: string;
  supabaseAnonKey: string;
  supabaseUserId: string;
  notifyChannel: string;
  notifyNtfyServer: string;
  notifyTelegramChatId: string;
  notifyMissedGraceMin: number;
}

export interface ConnectionTest {
  port: IntegrationPort;
  ok: boolean;
  code: string;
}

export type VerifyOutcome =
  | { status: 'signedIn'; userId: string }
  | { status: 'rejectedLocked' }
  | { status: 'invalidCode' }
  | { status: 'cooldown'; untilMs: number }
  | { status: 'rateLimited'; retryAfterMs: number };

export interface SignOutResult {
  deviceRemoved: boolean;
  serverSessionEnded: boolean;
}

/** Mirror and "PC is on" signal status. Codes only; `degradedCode` is `presence_unreachable` after repeated failed beats. */
export interface SyncStatus {
  active: boolean;
  lastBeatOkMs: number | null;
  lastSyncOkMs: number | null;
  beatError: string | null;
  syncError: string | null;
  degradedCode: string | null;
}

/** Computed on the PC from the topic secret; shown only on this screen. */
export interface NtfySubscription {
  deepLink: string;
  qrSvg: string;
}

export interface RemoteEvent {
  id: number;
  atMs: number;
  port: string;
  kind: string;
  outcome: string;
  httpStatus: number | null;
  bytes: number;
  detailCode: string | null;
}

export interface EgressSummary {
  port: string;
  kind: string;
  count: number;
  lastAtMs: number;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

const PORT_NAMES: readonly string[] = INTEGRATION_PORTS;

export function isHealth(value: unknown): value is Health {
  if (!isRecord(value)) return false;
  if (value.kind === 'ok' || value.kind === 'unknown') return true;
  return value.kind === 'degraded' && typeof value.code === 'string';
}

export function isIntegrationState(value: unknown): value is IntegrationState {
  if (!isRecord(value)) return false;
  return (
    typeof value.port === 'string' &&
    PORT_NAMES.includes(value.port) &&
    (value.setting === 'auto' || value.setting === 'mock') &&
    (value.mode === 'notConfigured' || value.mode === 'mock' || value.mode === 'live') &&
    typeof value.configured === 'boolean' &&
    isHealth(value.health) &&
    (value.lastOkMs === null || typeof value.lastOkMs === 'number')
  );
}

export function isIntegrationStatesView(value: unknown): value is IntegrationStatesView {
  if (!isRecord(value) || !Array.isArray(value.ports)) return false;
  const store = value.secretStore;
  const auth = value.auth;
  if (!isRecord(store) || !isRecord(auth) || !isRecord(store.configured)) return false;
  return (
    value.ports.every(isIntegrationState) &&
    (store.kind === 'keyring' || store.kind === 'inMemory' || store.kind === 'sessionOnly') &&
    (store.degradedCode === null || typeof store.degradedCode === 'string') &&
    typeof store.configured.anthropicApiKey === 'boolean' &&
    typeof store.configured.supabaseRefreshToken === 'boolean' &&
    typeof store.configured.ntfyTopic === 'boolean' &&
    typeof auth.signedIn === 'boolean' &&
    (auth.userId === null || typeof auth.userId === 'string') &&
    (auth.degradedCode === null || typeof auth.degradedCode === 'string') &&
    (auth.cooldownUntilMs === null || typeof auth.cooldownUntilMs === 'number')
  );
}

export function isRemoteConfig(value: unknown): value is RemoteConfig {
  if (!isRecord(value)) return false;
  return (
    typeof value.supabaseEndpoint === 'string' &&
    typeof value.supabaseAnonKey === 'string' &&
    typeof value.supabaseUserId === 'string' &&
    typeof value.notifyChannel === 'string' &&
    typeof value.notifyNtfyServer === 'string' &&
    typeof value.notifyTelegramChatId === 'string' &&
    typeof value.notifyMissedGraceMin === 'number'
  );
}

export function isVerifyOutcome(value: unknown): value is VerifyOutcome {
  if (!isRecord(value)) return false;
  switch (value.status) {
    case 'signedIn':
      return typeof value.userId === 'string';
    case 'rejectedLocked':
    case 'invalidCode':
      return true;
    case 'cooldown':
      return typeof value.untilMs === 'number';
    case 'rateLimited':
      return typeof value.retryAfterMs === 'number';
    default:
      return false;
  }
}

const nullableNumber = (v: unknown): boolean => v === null || typeof v === 'number';
const nullableString = (v: unknown): boolean => v === null || typeof v === 'string';

export function isSyncStatus(value: unknown): value is SyncStatus {
  if (!isRecord(value)) return false;
  return (
    typeof value.active === 'boolean' &&
    nullableNumber(value.lastBeatOkMs) &&
    nullableNumber(value.lastSyncOkMs) &&
    nullableString(value.beatError) &&
    nullableString(value.syncError) &&
    nullableString(value.degradedCode)
  );
}

export function isNtfySubscription(value: unknown): value is NtfySubscription {
  return isRecord(value) && typeof value.deepLink === 'string' && typeof value.qrSvg === 'string';
}

export function isRemoteEvent(value: unknown): value is RemoteEvent {
  if (!isRecord(value)) return false;
  return (
    typeof value.id === 'number' &&
    typeof value.atMs === 'number' &&
    typeof value.port === 'string' &&
    typeof value.kind === 'string' &&
    typeof value.outcome === 'string' &&
    (value.httpStatus === null || typeof value.httpStatus === 'number') &&
    typeof value.bytes === 'number' &&
    (value.detailCode === null || typeof value.detailCode === 'string')
  );
}

export const remoteCommands = {
  getIntegrationStates: () => invoke<IntegrationStatesView>('get_integration_states'),
  setIntegrationMode: (port: IntegrationPort, mode: ModeSetting) =>
    invoke<UpdateOutcome>('set_integration_mode', { port, mode }),
  getRemoteSettings: () => invoke<RemoteConfig>('get_remote_settings'),
  updateRemoteSetting: (key: RemoteSettingKey, value: string | number) =>
    invoke<UpdateOutcome>('update_remote_setting', { key, value }),
  /** The value is consumed by the core; the result never contains it. */
  saveSecret: (name: SecretName, value: string) => invoke<UpdateOutcome>('save_secret', { name, value }),
  clearSecret: (name: StoredSecretName) => invoke<UpdateOutcome>('clear_secret', { name }),
  testConnection: (port: IntegrationPort) => invoke<ConnectionTest>('test_connection', { port }),
  listRemoteEvents: (limit?: number) => invoke<RemoteEvent[]>('list_remote_events', { limit: limit ?? null }),
  getEgressSummary: () => invoke<EgressSummary[]>('get_egress_summary'),
  authRequestOtp: (email: string) => invoke<void>('auth_request_otp', { email }),
  authVerifyOtp: (email: string, code: string) => invoke<VerifyOutcome>('auth_verify_otp', { email, code }),
  getSyncStatus: () => invoke<SyncStatus>('get_sync_status'),
  /** Makes and stores a new random topic (a secret); the value is not returned. */
  generateNtfyTopic: () => invoke<UpdateOutcome>('generate_ntfy_topic'),
  getNtfySubscription: () => invoke<NtfySubscription | null>('get_ntfy_subscription'),
  testNotification: () => invoke<ConnectionTest>('test_notification'),
  authSignOut: () => invoke<SignOutResult>('auth_sign_out'),
};

export function onRemoteState(handler: (view: IntegrationStatesView) => void): Promise<UnlistenFn> {
  return listen<IntegrationStatesView>('remote://state', (event) => handler(event.payload));
}
