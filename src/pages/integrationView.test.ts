import { describe, expect, it } from 'vitest';
import contract from '../api/fixtures/remote-contract.json';
import {
  isIntegrationStatesView,
  isRemoteConfig,
  isRemoteEvent,
  isVerifyOutcome,
  type IntegrationState,
  type VerifyOutcome,
} from '../api';
import {
  authSummary,
  connectionMessage,
  draftAfterSave,
  eventKindLabel,
  healthText,
  modeBadge,
  saveSecretMessage,
  secretStoreNotice,
  verifyMessage,
} from './integrationView';

function state(overrides: Partial<IntegrationState>): IntegrationState {
  return {
    port: 'supabase',
    setting: 'auto',
    mode: 'live',
    configured: true,
    health: { kind: 'ok' },
    lastOkMs: null,
    ...overrides,
  };
}

// The same fixture is serialized from Rust in `commands::remote::tests::remote_wire_shapes_match_the_shared_fixture`.
describe('remote contract', () => {
  it('the shared shapes pass the production guards', () => {
    expect(isIntegrationStatesView(contract.view)).toBe(true);
    expect(isRemoteConfig(contract.settings)).toBe(true);
    expect(isRemoteEvent(contract.event)).toBe(true);
    expect(contract.verifyOutcomes.every((outcome: unknown) => isVerifyOutcome(outcome))).toBe(true);
  });

  it('rejects snake_case, unknown modes, and unknown ports', () => {
    expect(isIntegrationStatesView({ ...contract.view, secret_store: contract.view.secretStore, secretStore: undefined })).toBe(false);
    const [first, ...rest] = contract.view.ports;
    expect(isIntegrationStatesView({ ...contract.view, ports: [{ ...first, mode: 'live_mode' }, ...rest] })).toBe(false);
    expect(isIntegrationStatesView({ ...contract.view, ports: [{ ...first, port: 'telemetry' }, ...rest] })).toBe(false);
    expect(isVerifyOutcome({ status: 'signed_in', userId: 'u' })).toBe(false);
    expect(isVerifyOutcome({ status: 'cooldown' })).toBe(false);
  });
});

describe('mode badge', () => {
  it('shows the three states distinctly', () => {
    expect(modeBadge(state({ mode: 'notConfigured', configured: false }))).toEqual({ label: '설정 필요', tone: 'off' });
    expect(modeBadge(state({ mode: 'mock', setting: 'mock' }))).toEqual({ label: '모의 모드', tone: 'mock' });
    expect(modeBadge(state({}))).toEqual({ label: '실제 연결', tone: 'live' });
  });

  it('flags a degraded live port and never hides mock behind a health value', () => {
    expect(modeBadge(state({ health: { kind: 'degraded', code: 'auth_expired' } })).tone).toBe('warn');
    expect(modeBadge(state({ mode: 'mock', health: { kind: 'degraded', code: 'http_error' } })).label).toBe('모의 모드');
  });
});

describe('health and notices', () => {
  it('explains known codes and still reports unknown ones', () => {
    expect(healthText({ kind: 'degraded', code: 'auth_expired' })).toContain('다시 로그인');
    expect(healthText({ kind: 'degraded', code: 'weird_code' })).toContain('weird_code');
    expect(healthText({ kind: 'ok' })).toBeNull();
    expect(healthText({ kind: 'unknown' })).toBeNull();
  });

  it('a session-only keychain fallback produces a visible restart warning', () => {
    expect(secretStoreNotice(contract.view.secretStore as never)).toContain('다시 시작하면 사라집니다');
    expect(secretStoreNotice({ ...contract.view.secretStore, degradedCode: null } as never)).toBeNull();
  });

  it('summarizes the login', () => {
    expect(authSummary({ signedIn: true, userId: 'u', degradedCode: null, cooldownUntilMs: null })).toBe('로그인됨');
    expect(authSummary({ signedIn: false, userId: null, degradedCode: null, cooldownUntilMs: null })).toBe('로그인하지 않음');
    expect(authSummary(contract.view.auth)).toContain('만료');
  });
});

describe('sign-in messages', () => {
  it('counts the cooldown down from now and never shows less than one second', () => {
    const cooldown: VerifyOutcome = { status: 'cooldown', untilMs: 31_000 };
    expect(verifyMessage(cooldown, 1_000)).toContain('30초');
    expect(verifyMessage(cooldown, 30_999)).toContain('1초');
    expect(verifyMessage({ status: 'rejectedLocked' }, 0)).toContain('기록');
    expect(verifyMessage({ status: 'rateLimited', retryAfterMs: 42_000 }, 0)).toContain('42초');
  });
});

describe('secret entry', () => {
  it('empties the field after every attempt, accepted or refused', () => {
    expect(draftAfterSave()).toBe('');
  });

  it('says plainly when a run blocked the save', () => {
    expect(saveSecretMessage({ outcome: 'applied', key: 'ntfy_topic' })).toBe('저장했습니다.');
    expect(saveSecretMessage({ outcome: 'rejectedLocked', key: 'ntfy_topic' })).toContain('저장하지 않았습니다');
  });

  it('labels mock connection tests as sending nothing', () => {
    expect(connectionMessage(true, 'mock')).toContain('아무것도 보내지 않았습니다');
    expect(connectionMessage(false, 'timeout')).toContain('응답이 없습니다');
  });

  it('labels audit kinds and falls back to the raw kind', () => {
    expect(eventKindLabel('auth_verify')).toBe('로그인 코드 확인');
    expect(eventKindLabel('rpc_heartbeat')).toBe('rpc_heartbeat');
  });
});
