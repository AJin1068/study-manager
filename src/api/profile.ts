// Mirror of src-tauri/src/commands/profile.rs. Types match the Rust serde output and are checked
// against src/api/fixtures/profile-contract.json by both test suites.

import { invoke } from '@tauri-apps/api/core';

export type ProfileCode =
  | 'ok'
  | 'not_signed_in'
  | 'invalid_input'
  | 'not_shared'
  | 'not_found'
  | 'not_member'
  | 'cannot_self'
  | 'not_friends'
  | 'closed'
  | 'rate_limited'
  | 'key_conflict'
  | 'not_giftable'
  | 'daily_limit'
  | 'already_resolved'
  | 'name_required'
  | 'insufficient_points'
  | 'already_owned'
  | 'unknown_item'
  | 'too_many'
  | 'run_locked';

export const SECTIONS = ['room', 'grass', 'level', 'badges', 'guestbook', 'visitors', 'wiki', 'subject', 'gifts'] as const;
export type Section = (typeof SECTIONS)[number];

export const SCOPES = ['public', 'friends', 'private'] as const;
export type Scope = (typeof SCOPES)[number];

export interface ScopeRow {
  section: Section;
  scope: Scope;
}

export interface HomeView {
  name: string;
  slug: string | null;
  /** Shown only to the owner: anyone with the link can read the public sections. */
  link: string | null;
  webBaseUrl: string | null;
  scopes: ScopeRow[];
  wikiTitles: string[];
  giftDailyLimit: number;
  giftMessageMax: number;
}

export interface Ack {
  code: ProfileCode;
}

export interface PublishView {
  code: ProfileCode;
  slug: string | null;
  link: string | null;
}

export interface RoomLook {
  species: string | null;
  hat: string | null;
  top: string | null;
  accessory: string | null;
  hand: string | null;
  furniture: string[];
}

export interface GrassDay {
  day: string;
  studyMin: number;
}

export interface GuestEntry {
  id: number;
  authorName: string;
  body: string;
  atMs: number;
  mine: boolean;
  reported: boolean;
}

export interface Visitors {
  total: number;
  today: number;
}

export interface GiftedItem {
  itemId: string;
  senderName: string;
  message: string;
  atMs: number | null;
}

/** A section that is `null` and absent from `visible` is hidden from this viewer. */
export interface ProfileView {
  code: ProfileCode;
  name: string | null;
  slug: string | null;
  visible: Section[];
  room: RoomLook | null;
  grass: GrassDay[] | null;
  level: number | null;
  badges: string[] | null;
  guestbook: GuestEntry[] | null;
  visitors: Visitors | null;
  wiki: string[] | null;
  subject: string | null;
  gifts: GiftedItem[] | null;
}

export type OutboxState = 'pending' | 'sent' | 'refunded';

export interface SendOutcome {
  code: ProfileCode;
  giftId: string | null;
  state: OutboxState | null;
  balance: number;
}

export interface ReceiveOutcome {
  code: ProfileCode;
  itemId: string | null;
  alreadyOwned: boolean;
}

export interface OutboxView {
  giftId: string;
  recipientName: string;
  itemId: string;
  message: string;
  price: number;
  state: OutboxState;
  createdAtMs: number;
}

export interface InboxGift {
  giftId: string;
  itemId: string;
  senderName: string;
  message: string;
  status: string;
  atMs: number;
}

export interface GiftOverview {
  outbox: OutboxView[];
  inbox: InboxGift[];
  online: boolean;
}

const CODES: readonly string[] = [
  'ok', 'not_signed_in', 'invalid_input', 'not_shared', 'not_found', 'not_member', 'cannot_self', 'not_friends',
  'closed', 'rate_limited', 'key_conflict', 'not_giftable', 'daily_limit', 'already_resolved', 'name_required',
  'insufficient_points', 'already_owned', 'unknown_item', 'too_many', 'run_locked',
];
const OUTBOX_STATES: readonly string[] = ['pending', 'sent', 'refunded'];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

const isStr = (v: unknown): v is string => typeof v === 'string';
const isNum = (v: unknown): v is number => typeof v === 'number';
const orNull = <T>(v: unknown, guard: (x: unknown) => x is T): boolean => v === null || guard(v);
const isStrList = (v: unknown): v is string[] => Array.isArray(v) && v.every(isStr);
const listOf = <T>(guard: (x: unknown) => x is T) => (v: unknown): v is T[] => Array.isArray(v) && v.every(guard);

export function isProfileCode(value: unknown): value is ProfileCode {
  return isStr(value) && CODES.includes(value);
}

export function isSection(value: unknown): value is Section {
  return isStr(value) && (SECTIONS as readonly string[]).includes(value);
}

export function isScope(value: unknown): value is Scope {
  return isStr(value) && (SCOPES as readonly string[]).includes(value);
}

export function isHomeView(value: unknown): value is HomeView {
  return (
    isRecord(value) &&
    isStr(value.name) &&
    orNull(value.slug, isStr) &&
    orNull(value.link, isStr) &&
    orNull(value.webBaseUrl, isStr) &&
    Array.isArray(value.scopes) &&
    value.scopes.every((r) => isRecord(r) && isSection(r.section) && isScope(r.scope)) &&
    isStrList(value.wikiTitles) &&
    isNum(value.giftDailyLimit) &&
    isNum(value.giftMessageMax)
  );
}

export function isAck(value: unknown): value is Ack {
  return isRecord(value) && isProfileCode(value.code);
}

export function isPublishView(value: unknown): value is PublishView {
  return isRecord(value) && isProfileCode(value.code) && orNull(value.slug, isStr) && orNull(value.link, isStr);
}

const isRoomLook = (v: unknown): v is RoomLook =>
  isRecord(v) &&
  [v.species, v.hat, v.top, v.accessory, v.hand].every((x) => orNull(x, isStr)) &&
  isStrList(v.furniture);

const isGrassDay = (v: unknown): v is GrassDay => isRecord(v) && isStr(v.day) && isNum(v.studyMin);

const isGuestEntry = (v: unknown): v is GuestEntry =>
  isRecord(v) && isNum(v.id) && isStr(v.authorName) && isStr(v.body) && isNum(v.atMs) && typeof v.mine === 'boolean' && typeof v.reported === 'boolean';

const isVisitors = (v: unknown): v is Visitors => isRecord(v) && isNum(v.total) && isNum(v.today);

const isGiftedItem = (v: unknown): v is GiftedItem =>
  isRecord(v) && isStr(v.itemId) && isStr(v.senderName) && isStr(v.message) && orNull(v.atMs, isNum);

export function isProfileView(value: unknown): value is ProfileView {
  return (
    isRecord(value) &&
    isProfileCode(value.code) &&
    orNull(value.name, isStr) &&
    orNull(value.slug, isStr) &&
    Array.isArray(value.visible) &&
    value.visible.every(isSection) &&
    orNull(value.room, isRoomLook) &&
    orNull(value.grass, listOf(isGrassDay)) &&
    orNull(value.level, isNum) &&
    orNull(value.badges, isStrList) &&
    orNull(value.guestbook, listOf(isGuestEntry)) &&
    orNull(value.visitors, isVisitors) &&
    orNull(value.wiki, isStrList) &&
    orNull(value.subject, isStr) &&
    orNull(value.gifts, listOf(isGiftedItem))
  );
}

export function isSendOutcome(value: unknown): value is SendOutcome {
  return (
    isRecord(value) &&
    isProfileCode(value.code) &&
    orNull(value.giftId, isStr) &&
    (value.state === null || (isStr(value.state) && OUTBOX_STATES.includes(value.state))) &&
    isNum(value.balance)
  );
}

export function isReceiveOutcome(value: unknown): value is ReceiveOutcome {
  return isRecord(value) && isProfileCode(value.code) && orNull(value.itemId, isStr) && typeof value.alreadyOwned === 'boolean';
}

const isOutboxView = (v: unknown): v is OutboxView =>
  isRecord(v) &&
  isStr(v.giftId) &&
  isStr(v.recipientName) &&
  isStr(v.itemId) &&
  isStr(v.message) &&
  isNum(v.price) &&
  isStr(v.state) &&
  OUTBOX_STATES.includes(v.state) &&
  isNum(v.createdAtMs);

const isInboxGift = (v: unknown): v is InboxGift =>
  isRecord(v) && isStr(v.giftId) && isStr(v.itemId) && isStr(v.senderName) && isStr(v.message) && isStr(v.status) && isNum(v.atMs);

export function isGiftOverview(value: unknown): value is GiftOverview {
  return (
    isRecord(value) &&
    listOf(isOutboxView)(value.outbox) &&
    listOf(isInboxGift)(value.inbox) &&
    typeof value.online === 'boolean'
  );
}

export const profileCommands = {
  profileHome: () => invoke<HomeView>('profile_home'),
  profileSetName: (name: string) => invoke<Ack>('profile_set_name', { name }),
  /** Saves the scope, then publishes so a narrowed scope takes effect on the server at once. */
  profileSetScope: (section: Section, scope: Scope) => invoke<PublishView>('profile_set_scope', { section, scope }),
  profileSetWikiTitles: (titles: string[]) => invoke<Ack>('profile_set_wiki_titles', { titles }),
  profileSetWebBase: (base: string) => invoke<Ack>('profile_set_web_base', { base }),
  profilePublish: () => invoke<PublishView>('profile_publish'),
  profileMyView: () => invoke<ProfileView>('profile_my_view'),
  profileMemberView: (userId: string) => invoke<ProfileView>('profile_member_view', { userId }),
  profileSignGuestbook: (ownerId: string, body: string) => invoke<Ack>('profile_sign_guestbook', { ownerId, body }),
  profileDeleteEntry: (id: number) => invoke<Ack>('profile_delete_entry', { id }),
  profileReportEntry: (id: number) => invoke<Ack>('profile_report_entry', { id }),
  giftSend: (recipientId: string, recipientName: string, itemId: string, message: string) =>
    invoke<SendOutcome>('gift_send', { recipientId, recipientName, itemId, message }),
  giftOverview: () => invoke<GiftOverview>('gift_overview'),
  giftAccept: (giftId: string) => invoke<ReceiveOutcome>('gift_accept', { giftId }),
  giftDecline: (giftId: string) => invoke<Ack>('gift_decline', { giftId }),
};
