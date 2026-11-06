// Mirror of src-tauri/src/commands/rooms.rs. Types match the Rust serde output and are checked
// against src/api/fixtures/rooms-contract.json by both test suites.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { UpdateOutcome } from './settings';

export type RoomCode =
  | 'ok'
  | 'not_signed_in'
  | 'invalid_input'
  | 'room_limit'
  | 'not_owner'
  | 'too_many_invites'
  | 'not_found'
  | 'rate_limited'
  | 'invalid_code'
  | 'revoked'
  | 'expired'
  | 'exhausted'
  | 'banned'
  | 'already_member'
  | 'room_full'
  | 'not_member'
  | 'cannot_kick_self'
  | 'not_shared'
  | 'surge_rejected'
  | 'disabled'
  | 'throttled';

export type RoomRole = 'owner' | 'member';
export type PresenceKind = 'studying' | 'break' | 'idle' | 'offline' | 'hidden';

export interface RoomSummary {
  roomId: string;
  name: string;
  role: RoomRole;
  memberCount: number;
  capacity: number;
}

export interface RoomList {
  rooms: RoomSummary[];
  /** True when the server could not be reached and these rows are the last known ones. */
  stale: boolean;
  refreshedAtMs: number | null;
}

export interface RoomAction {
  code: RoomCode;
  roomId: string | null;
}

export interface InviteOutcome {
  code: RoomCode;
  inviteId: string | null;
  /** Shown once; never stored by the UI. */
  inviteCode: string | null;
  expiresAtMs: number | null;
  deepLink: string | null;
  qrSvg: string | null;
}

export interface InvitePayload {
  code: string;
  endpoint: string | null;
}

export interface PresenceView {
  kind: PresenceKind;
  remainingSec: number | null;
}

export interface RoomCharacterView {
  species: string;
  hat: string | null;
  top: string | null;
  accessory: string | null;
  hand: string | null;
}

export interface MemberView {
  userId: string;
  nickname: string;
  role: RoomRole;
  isSelf: boolean;
  level: number | null;
  presence: PresenceView;
  attemptsToday: number | null;
  character: RoomCharacterView | null;
}

export interface RankRow {
  userId: string;
  nickname: string;
  isSelf: boolean;
  /** Null when the member does not share study time. */
  minutes: number | null;
  rank: number | null;
}

export interface InviteView {
  inviteId: string;
  expiresAtMs: number;
  maxUses: number;
  uses: number;
  revoked: boolean;
  expired: boolean;
}

export interface TogetherItem {
  key: string;
  name: string;
  subject: string;
  sets: number;
  startAtUtcMs: number;
  localDate: string;
  localTime: string;
  alreadyImported: boolean;
  inPast: boolean;
}

export interface RoomView {
  roomId: string;
  name: string;
  role: RoomRole;
  capacity: number;
  members: MemberView[];
  todayRanking: RankRow[];
  weekRanking: RankRow[];
  invites: InviteView[];
  scheduleUpdatedAtMs: number | null;
  schedule: TogetherItem[];
  fetchedAtMs: number;
}

export interface FeedView {
  room: RoomView | null;
  /** A code such as `timeout` when the latest refresh failed; `room` is then the previous good one. */
  degradedCode: string | null;
  lastOkMs: number | null;
  pollIntervalMs: number;
}

export interface FriendSettings {
  shareLevel: boolean;
  shareStatus: boolean;
  shareWeekTime: boolean;
  shareAttempts: boolean;
  distractionPublish: boolean;
  distractionReceive: boolean;
}

export type FriendSettingKey = keyof FriendSettings;

export type ImportOutcome = 'applied' | 'alreadyImported' | 'inPast' | 'notShared' | 'rejectedLocked' | 'invalid';

export interface ImportResult {
  key: string;
  outcome: ImportOutcome;
  planId: number | null;
}

export interface DistractionNotice {
  nickname: string;
  atMs: number;
}

export const ROOM_NOTICE_EVENT = 'rooms://distraction';

const CODES: readonly string[] = [
  'ok', 'not_signed_in', 'invalid_input', 'room_limit', 'not_owner', 'too_many_invites', 'not_found', 'rate_limited',
  'invalid_code', 'revoked', 'expired', 'exhausted', 'banned', 'already_member', 'room_full', 'not_member',
  'cannot_kick_self', 'not_shared', 'surge_rejected', 'disabled', 'throttled',
];
const PRESENCE: readonly string[] = ['studying', 'break', 'idle', 'offline', 'hidden'];
const IMPORT_OUTCOMES: readonly string[] = ['applied', 'alreadyImported', 'inPast', 'notShared', 'rejectedLocked', 'invalid'];

type Rec = Record<string, unknown>;

function rec(value: unknown): value is Rec {
  return typeof value === 'object' && value !== null;
}
const str = (v: unknown): v is string => typeof v === 'string';
const num = (v: unknown): v is number => typeof v === 'number';
const strOrNull = (v: unknown): v is string | null => v === null || str(v);
const numOrNull = (v: unknown): v is number | null => v === null || num(v);
const role = (v: unknown): v is RoomRole => v === 'owner' || v === 'member';
const list = <T>(v: unknown, guard: (item: unknown) => item is T): v is T[] => Array.isArray(v) && v.every(guard);

export function isRoomCode(value: unknown): value is RoomCode {
  return str(value) && CODES.includes(value);
}

export function isRoomSummary(value: unknown): value is RoomSummary {
  return rec(value) && str(value.roomId) && str(value.name) && role(value.role) && num(value.memberCount) && num(value.capacity);
}

export function isRoomList(value: unknown): value is RoomList {
  return rec(value) && list(value.rooms, isRoomSummary) && typeof value.stale === 'boolean' && numOrNull(value.refreshedAtMs);
}

export function isRoomAction(value: unknown): value is RoomAction {
  return rec(value) && isRoomCode(value.code) && strOrNull(value.roomId);
}

export function isInviteOutcome(value: unknown): value is InviteOutcome {
  return (
    rec(value) &&
    isRoomCode(value.code) &&
    strOrNull(value.inviteId) &&
    strOrNull(value.inviteCode) &&
    numOrNull(value.expiresAtMs) &&
    strOrNull(value.deepLink) &&
    strOrNull(value.qrSvg)
  );
}

function isPresence(value: unknown): value is PresenceView {
  return rec(value) && str(value.kind) && PRESENCE.includes(value.kind) && numOrNull(value.remainingSec);
}

function isCharacter(value: unknown): value is RoomCharacterView {
  return rec(value) && str(value.species) && strOrNull(value.hat) && strOrNull(value.top) && strOrNull(value.accessory) && strOrNull(value.hand);
}

function isMember(value: unknown): value is MemberView {
  return (
    rec(value) &&
    str(value.userId) &&
    str(value.nickname) &&
    role(value.role) &&
    typeof value.isSelf === 'boolean' &&
    numOrNull(value.level) &&
    isPresence(value.presence) &&
    numOrNull(value.attemptsToday) &&
    (value.character === null || isCharacter(value.character))
  );
}

function isRank(value: unknown): value is RankRow {
  return rec(value) && str(value.userId) && str(value.nickname) && typeof value.isSelf === 'boolean' && numOrNull(value.minutes) && numOrNull(value.rank);
}

function isInviteView(value: unknown): value is InviteView {
  return (
    rec(value) &&
    str(value.inviteId) &&
    num(value.expiresAtMs) &&
    num(value.maxUses) &&
    num(value.uses) &&
    typeof value.revoked === 'boolean' &&
    typeof value.expired === 'boolean'
  );
}

function isTogether(value: unknown): value is TogetherItem {
  return (
    rec(value) &&
    str(value.key) &&
    str(value.name) &&
    str(value.subject) &&
    num(value.sets) &&
    num(value.startAtUtcMs) &&
    str(value.localDate) &&
    str(value.localTime) &&
    typeof value.alreadyImported === 'boolean' &&
    typeof value.inPast === 'boolean'
  );
}

export function isRoomView(value: unknown): value is RoomView {
  return (
    rec(value) &&
    str(value.roomId) &&
    str(value.name) &&
    role(value.role) &&
    num(value.capacity) &&
    list(value.members, isMember) &&
    list(value.todayRanking, isRank) &&
    list(value.weekRanking, isRank) &&
    list(value.invites, isInviteView) &&
    numOrNull(value.scheduleUpdatedAtMs) &&
    list(value.schedule, isTogether) &&
    num(value.fetchedAtMs)
  );
}

export function isFeedView(value: unknown): value is FeedView {
  return rec(value) && (value.room === null || isRoomView(value.room)) && strOrNull(value.degradedCode) && numOrNull(value.lastOkMs) && num(value.pollIntervalMs);
}

export function isFriendSettings(value: unknown): value is FriendSettings {
  return (
    rec(value) &&
    typeof value.shareLevel === 'boolean' &&
    typeof value.shareStatus === 'boolean' &&
    typeof value.shareWeekTime === 'boolean' &&
    typeof value.shareAttempts === 'boolean' &&
    typeof value.distractionPublish === 'boolean' &&
    typeof value.distractionReceive === 'boolean'
  );
}

export function isImportResult(value: unknown): value is ImportResult {
  return rec(value) && str(value.key) && str(value.outcome) && IMPORT_OUTCOMES.includes(value.outcome) && numOrNull(value.planId);
}

export function isDistractionNotice(value: unknown): value is DistractionNotice {
  return rec(value) && str(value.nickname) && num(value.atMs);
}

export const roomCommands = {
  listRooms: () => invoke<RoomList>('rooms_list'),
  createRoom: (name: string, nickname: string) => invoke<RoomAction>('rooms_create', { name, nickname }),
  /** `acknowledged` is the user's confirmation of the sharing notice shown before joining. */
  joinRoom: (code: string, nickname: string, acknowledged: boolean) => invoke<RoomAction>('rooms_join', { code, nickname, acknowledged }),
  leaveRoom: (roomId: string) => invoke<RoomAction>('rooms_leave', { roomId }),
  kickMember: (roomId: string, userId: string) => invoke<RoomAction>('rooms_kick', { roomId, userId }),
  createInvite: (roomId: string) => invoke<InviteOutcome>('rooms_create_invite', { roomId }),
  revokeInvite: (roomId: string, inviteId: string) => invoke<RoomAction>('rooms_revoke_invite', { roomId, inviteId }),
  parseInvite: (text: string) => invoke<InvitePayload | null>('rooms_parse_invite', { text }),
  openRoom: (roomId: string) => invoke<void>('rooms_open', { roomId }),
  closeRoom: () => invoke<void>('rooms_close'),
  getRoomFeed: () => invoke<FeedView>('rooms_feed'),
  getFriendSettings: () => invoke<FriendSettings>('rooms_friend_settings'),
  updateFriendSetting: (key: FriendSettingKey, value: boolean) => invoke<UpdateOutcome>('rooms_update_friend_setting', { key, value }),
  shareSchedule: (roomId: string) => invoke<RoomAction>('rooms_share_schedule', { roomId }),
  importTogether: (roomId: string, keys: string[]) => invoke<ImportResult[]>('rooms_import_together', { roomId, keys }),
};

/** A friend's content-free distraction notice (only when this user turned receiving on). */
export function onDistractionNotice(handler: (notice: DistractionNotice) => void): Promise<UnlistenFn> {
  return listen<DistractionNotice>(ROOM_NOTICE_EVENT, (event) => handler(event.payload));
}
