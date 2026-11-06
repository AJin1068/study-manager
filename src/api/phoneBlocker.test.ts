import { describe, expect, it } from 'vitest';
import contract from '../../fixtures/phone/blocker-bridge.json';
import { isBlockerStatus } from './phoneInstall';
import {
  ROLE_EMERGENCY_CALL,
  applyBridgeEvent,
  bridgeResult,
  engageRequest,
  isBridgeEvent,
  normalizeAllowList,
  type LockLeaseWire,
  type PhoneBeatWire,
} from './phoneBlocker';

// The same fixture is read by src-tauri/src/phone/{allow_list,blocker_port,android_blocker}.rs.
describe('blocker allow list', () => {
  it('normalizes the fixture inputs to the shared wire form', () => {
    for (const c of contract.allowList.valid) {
      expect(normalizeAllowList(c.input), c.name).toEqual({ ok: true, value: c.wire });
    }
  });

  it('refuses the fixture inputs with the shared codes', () => {
    for (const c of contract.allowList.invalid) {
      expect(normalizeAllowList(c.input), c.name).toEqual({ ok: false, error: c.error });
    }
  });

  it('always contains the emergency call', () => {
    expect(contract.allowList.emergencyRole).toBe(ROLE_EMERGENCY_CALL);
    for (const roles of [[], ['dialer'], ['messages', 'dialer']]) {
      const result = normalizeAllowList({ roles, packages: [] });
      expect(result.ok && result.value.roles[0]).toBe(ROLE_EMERGENCY_CALL);
    }
  });

  it('caps the custom packages', () => {
    const many = Array.from({ length: contract.allowList.maxCustomPackages + 1 }, (_, n) => `com.example.app${n}`);
    expect(normalizeAllowList({ roles: [], packages: many })).toEqual({ ok: false, error: 'too_many_packages' });
    expect(normalizeAllowList({ roles: [], packages: many.slice(1) }).ok).toBe(true);
  });
});

describe('lease -> engage mapping', () => {
  it('matches the fixture cases', () => {
    const allow = { roles: ['emergency_call' as const], packages: [] };
    for (const c of contract.leaseMapping) {
      const result = engageRequest(c.lease as LockLeaseWire, c.remaining_ms, allow);
      if ('run_id' in c.expect) {
        expect(result, c.name).toEqual({ ok: true, value: { ...c.expect, allow } });
      } else {
        expect(result, c.name).toEqual({ ok: false, error: c.expect.error });
      }
    }
  });
});

describe('plugin replies and events', () => {
  it('maps engage and release replies like the Rust adapter', () => {
    for (const command of [contract.commands.engage, contract.commands.release]) {
      for (const c of command.replies) {
        const expected = c.result === 'ok' ? 'ok' : c.result;
        expect(bridgeResult(c.reply), c.name).toBe(expected);
      }
    }
  });

  it('treats replies outside the contract as malformed', () => {
    for (const reply of [{}, { ok: true, code: 'internal' }, { ok: false }, { ok: true, extra: 1 }, 'ok', null]) {
      expect(bridgeResult(reply)).toBe('bridge_malformed');
    }
  });

  it('status replies are valid blocker statuses, or become unreachable', () => {
    for (const c of contract.commands.status.replies) {
      expect(isBlockerStatus(c.status), c.name).toBe(true);
    }
  });

  it('turns events into the next heartbeat like the fixture says', () => {
    for (const c of contract.events) {
      expect(isBridgeEvent(c.event), c.name).toBe(true);
      expect(applyBridgeEvent(c.event as never, c.before as PhoneBeatWire), c.name).toEqual(c.after);
    }
    expect(isBridgeEvent({ event: 'package_opened' })).toBe(false);
    expect(isBridgeEvent({ event: 'lock_ended', package: 'com.example' })).toBe(false);
  });

  it('has no field that could name an app the user opened', () => {
    const text = JSON.stringify([contract.commands.engage.request, contract.events]);
    for (const word of ['package_name', 'app_name', 'foreground', 'usage', 'title', 'url']) expect(text).not.toContain(word);
  });
});
