// Mirror of src-tauri/src/commands/phone.rs. Types match the Rust serde output and are checked
// against src/api/fixtures/phone-contract.json by both test suites.

import { invoke } from '@tauri-apps/api/core';

export const PHONE_CODES = [
  'ok',
  'not_signed_in',
  'invalid_input',
  'anonymous_not_allowed',
  'already_paired',
  'rate_limited',
  'invalid_code',
  'used',
  'expired',
  'cannot_self',
  'phone_already_paired',
  'not_found',
  'not_paired',
  'stale_seq',
  'lease_too_long',
  'run_locked',
  'bad_offer',
] as const;
export type PhoneCode = (typeof PHONE_CODES)[number];

export interface PairStart {
  code: PhoneCode;
  /** `ABCDE-FGHJK`, to type on the phone when scanning is not possible. */
  displayCode: string | null;
  expiresAtMs: number | null;
  qrSvg: string | null;
}

export interface PairStatusView {
  paired: boolean;
  pairedAtMs: number | null;
  lastBeatMs: number | null;
  /** `false` when the phone reported its blocking service switched off. */
  serviceEnabled: boolean | null;
  /** Whether the server could be reached; when not, the rest is the last known state. */
  online: boolean;
}

export const GAP_KINDS = ['unreachable', 'lease_expired', 'service_disabled'] as const;
export type GapKind = (typeof GAP_KINDS)[number];

export interface PublishedLease {
  seq: number;
  state: 'locked' | 'free';
  /** Why a free lease is free; `null` for a locked one. */
  reason: string | null;
  atMs: number;
}

export interface OpenGap {
  kind: GapKind;
  sinceMs: number;
}

/** The link while a lock is wanted: whether the lock reached the server, gaps, last beat. */
export interface PhoneLinkView {
  paired: boolean;
  /** A study segment holds the lock, so the phone should be locked now. */
  wantLocked: boolean;
  published: PublishedLease | null;
  /** Publications failed in a row: the server could not be reached with the lock. */
  publishFailing: boolean;
  lastError: string | null;
  /** Gaps happening now; a row is written when each ends. */
  openGaps: OpenGap[];
  lastBeatMs: number | null;
  serviceEnabled: boolean | null;
}

export interface PhoneAck {
  code: PhoneCode;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

const isStr = (v: unknown): v is string => typeof v === 'string';
const isNum = (v: unknown): v is number => typeof v === 'number';
const isBool = (v: unknown): v is boolean => typeof v === 'boolean';
const orNull = <T>(v: unknown, guard: (x: unknown) => x is T): boolean => v === null || guard(v);

export function isPhoneCode(value: unknown): value is PhoneCode {
  return isStr(value) && (PHONE_CODES as readonly string[]).includes(value);
}

export function isPairStart(value: unknown): value is PairStart {
  return (
    isRecord(value) &&
    isPhoneCode(value.code) &&
    orNull(value.displayCode, isStr) &&
    orNull(value.expiresAtMs, isNum) &&
    orNull(value.qrSvg, isStr)
  );
}

export function isPairStatusView(value: unknown): value is PairStatusView {
  return (
    isRecord(value) &&
    isBool(value.paired) &&
    orNull(value.pairedAtMs, isNum) &&
    orNull(value.lastBeatMs, isNum) &&
    orNull(value.serviceEnabled, isBool) &&
    isBool(value.online)
  );
}

const isGapKind = (v: unknown): v is GapKind => isStr(v) && (GAP_KINDS as readonly string[]).includes(v);

function isPublishedLease(value: unknown): value is PublishedLease {
  return (
    isRecord(value) &&
    isNum(value.seq) &&
    (value.state === 'locked' || value.state === 'free') &&
    orNull(value.reason, isStr) &&
    isNum(value.atMs)
  );
}

function isOpenGap(value: unknown): value is OpenGap {
  return isRecord(value) && isGapKind(value.kind) && isNum(value.sinceMs);
}

export function isPhoneLinkView(value: unknown): value is PhoneLinkView {
  return (
    isRecord(value) &&
    isBool(value.paired) &&
    isBool(value.wantLocked) &&
    (value.published === null || isPublishedLease(value.published)) &&
    isBool(value.publishFailing) &&
    orNull(value.lastError, isStr) &&
    Array.isArray(value.openGaps) &&
    value.openGaps.every(isOpenGap) &&
    orNull(value.lastBeatMs, isNum) &&
    orNull(value.serviceEnabled, isBool)
  );
}

export function isPhoneAck(value: unknown): value is PhoneAck {
  return isRecord(value) && isPhoneCode(value.code);
}

export const phoneCommands = {
  /** A new one-time code and its QR code (valid 5 minutes, one use). */
  phonePairStart: () => invoke<PairStart>('phone_pair_start'),
  phonePairStatus: () => invoke<PairStatusView>('phone_pair_status'),
  /** Ends the pairing. Refused with `run_locked` while a study run holds the lock. */
  phonePairRevoke: () => invoke<PhoneAck>('phone_pair_revoke'),
  /** What the publisher knows about the link; makes no network call. */
  getPhoneLink: () => invoke<PhoneLinkView>('get_phone_link'),
};
