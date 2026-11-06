// Mirror of the blocker plugin contract in src-tauri/src/phone/{allow_list,blocker_port,android_blocker}.rs,
// checked against fixtures/phone/blocker-bridge.json by both test suites (and by the Kotlin plugin).
// Pure functions only: the phone project owns the plugin call itself. The allow list stays on the
// phone; nothing here is sent to the server, and no type has a field for an app the user opened.

export const ROLE_EMERGENCY_CALL = 'emergency_call';
export const MAX_CUSTOM_PACKAGES = 50;
export const MAX_LEASE_MS = 3 * 60 * 60 * 1000 + 60_000;

const ROLES = ['emergency_call', 'dialer', 'messages'] as const;
export type AllowRole = (typeof ROLES)[number];

export interface AllowListWire {
  roles: AllowRole[];
  packages: string[];
}

export type AllowListErrorCode = 'malformed' | 'unknown_role' | 'invalid_package' | 'settings_package' | 'too_many_packages';
export type AllowListResult = { ok: true; value: AllowListWire } | { ok: false; error: AllowListErrorCode };

const SETTINGS_PREFIXES = ['com.android.settings', 'com.android.packageinstaller', 'com.google.android.packageinstaller'];
const SETTINGS_PACKAGES = ['com.google.android.permissioncontroller', 'com.android.permissioncontroller'];
const PACKAGE_SEGMENT = /^[A-Za-z][A-Za-z0-9_]*$/;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function packageError(name: string): AllowListErrorCode | null {
  const segments = name.split('.');
  if (name.length > 255 || segments.length < 2 || !segments.every((s) => PACKAGE_SEGMENT.test(s))) return 'invalid_package';
  const lowered = name.toLowerCase();
  if (SETTINGS_PACKAGES.includes(lowered) || SETTINGS_PREFIXES.some((p) => lowered.startsWith(p))) return 'settings_package';
  return null;
}

/**
 * Normalizes an allow list the way the Rust side does: the emergency call is always present (added
 * when missing), roles come out in a fixed order, packages sorted and deduplicated, and the
 * settings app can never be allowed.
 */
export function normalizeAllowList(input: unknown): AllowListResult {
  if (!isRecord(input) || Object.keys(input).some((k) => k !== 'roles' && k !== 'packages')) return { ok: false, error: 'malformed' };
  const { roles, packages } = input;
  if (!Array.isArray(roles) || !Array.isArray(packages)) return { ok: false, error: 'malformed' };
  const wanted = new Set<string>();
  for (const role of roles) {
    if (typeof role !== 'string') return { ok: false, error: 'malformed' };
    if (!(ROLES as readonly string[]).includes(role)) return { ok: false, error: 'unknown_role' };
    wanted.add(role);
  }
  const names = new Set<string>();
  for (const name of packages) {
    if (typeof name !== 'string') return { ok: false, error: 'malformed' };
    const error = packageError(name);
    if (error) return { ok: false, error };
    names.add(name);
  }
  if (names.size > MAX_CUSTOM_PACKAGES) return { ok: false, error: 'too_many_packages' };
  return {
    ok: true,
    value: {
      roles: ROLES.filter((role) => role === ROLE_EMERGENCY_CALL || wanted.has(role)),
      packages: [...names].sort(),
    },
  };
}

export interface LockLeaseWire {
  run_id: string;
  seq: number;
  state: 'locked' | 'free';
  release_cause: string | null;
  issued_at_ms: number;
  valid_until_ms: number;
}

export interface EngageRequest {
  run_id: string;
  seq: number;
  remaining_ms: number;
  allow: AllowListWire;
}

export type EngageError = 'not_locked' | 'bad_remaining';

/** The lease -> engage mapping: only a locked lease with 1..=MAX_LEASE_MS left can be engaged. */
export function engageRequest(
  lease: LockLeaseWire,
  remainingMs: number,
  allow: AllowListWire,
): { ok: true; value: EngageRequest } | { ok: false; error: EngageError } {
  if (lease.state !== 'locked' || lease.release_cause !== null || lease.valid_until_ms <= lease.issued_at_ms) {
    return { ok: false, error: 'not_locked' };
  }
  if (!Number.isInteger(remainingMs) || remainingMs < 1 || remainingMs > MAX_LEASE_MS) return { ok: false, error: 'bad_remaining' };
  return { ok: true, value: { run_id: lease.run_id, seq: lease.seq, remaining_ms: remainingMs, allow } };
}

export type BridgeResult = 'ok' | 'service_disabled' | 'refused' | 'bridge_malformed';

/** Maps a plugin reply to the result code the Rust adapter reports. */
export function bridgeResult(reply: unknown): BridgeResult {
  if (!isRecord(reply) || Object.keys(reply).some((k) => k !== 'ok' && k !== 'code') || typeof reply.ok !== 'boolean') return 'bridge_malformed';
  if (reply.ok) return reply.code === undefined ? 'ok' : 'bridge_malformed';
  if (reply.code === 'service_disabled') return 'service_disabled';
  return reply.code === 'invalid_request' || reply.code === 'internal' ? 'refused' : 'bridge_malformed';
}

export interface PhoneBeatWire {
  lease_seq: number;
  locked: boolean;
  service_enabled: boolean;
}

export type BridgeEvent = { event: 'service_disabled' } | { event: 'service_enabled' } | { event: 'lock_ended' };

export function isBridgeEvent(value: unknown): value is BridgeEvent {
  return (
    isRecord(value) &&
    Object.keys(value).length === 1 &&
    (value.event === 'service_disabled' || value.event === 'service_enabled' || value.event === 'lock_ended')
  );
}

/** The heartbeat that follows `beat` after the event. */
export function applyBridgeEvent(event: BridgeEvent, beat: PhoneBeatWire): PhoneBeatWire {
  switch (event.event) {
    case 'service_disabled':
      return { ...beat, service_enabled: false };
    case 'service_enabled':
      return { ...beat, service_enabled: true };
    case 'lock_ended':
      return { ...beat, locked: false };
  }
}
