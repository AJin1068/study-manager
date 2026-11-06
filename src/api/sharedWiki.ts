// Mirror of src-tauri/src/commands/shared_wiki.rs. Types match the Rust serde output and are
// checked against src/api/fixtures/shared-wiki-contract.json by both test suites.

import { invoke } from '@tauri-apps/api/core';
import type { QuizItemKind } from './quiz';

export interface SharePreview {
  pageName: string;
  preview: string;
  memberCount: number | null;
  shareNotice: string;
  mergeNotice: string;
}

/** `code` is `ok`, `not_confirmed` or a server code such as `not_member` or `too_large`. */
export interface SharedAction {
  code: string;
  branched: boolean;
}

export interface SharedSync {
  pushed: number;
  failed: number;
}

export interface RoomPageView {
  title: string;
  contributors: string[];
  hasMerge: boolean;
  branched: boolean;
  sharedByMe: boolean;
  mergeableParts: number;
  listedOnly: string[];
}

export interface NewConceptView {
  title: string;
  authors: string[];
}

export interface SharedRoomView {
  pages: RoomPageView[];
  newConcepts: NewConceptView[];
  summary: string;
  shareNotice: string;
}

export interface SharedMerge {
  code: string;
  usedLlm: boolean;
  fallbackReason: string | null;
  contributors: string[];
  listedOnly: string[];
  markdown: string;
}

export interface SharedQuizQuestion {
  itemId: number;
  kind: QuizItemKind;
  prompt: string;
  choices: string[];
  pageName: string;
}

type Rec = Record<string, unknown>;

function rec(value: unknown): value is Rec {
  return typeof value === 'object' && value !== null;
}
const str = (v: unknown): v is string => typeof v === 'string';
const num = (v: unknown): v is number => typeof v === 'number';
const bool = (v: unknown): v is boolean => typeof v === 'boolean';
const strings = (v: unknown): v is string[] => Array.isArray(v) && v.every(str);
const list = <T>(v: unknown, guard: (item: unknown) => item is T): v is T[] => Array.isArray(v) && v.every(guard);

export function isSharePreview(value: unknown): value is SharePreview {
  return (
    rec(value) &&
    str(value.pageName) &&
    str(value.preview) &&
    (value.memberCount === null || num(value.memberCount)) &&
    str(value.shareNotice) &&
    str(value.mergeNotice)
  );
}

export function isSharedAction(value: unknown): value is SharedAction {
  return rec(value) && str(value.code) && bool(value.branched);
}

export function isSharedSync(value: unknown): value is SharedSync {
  return rec(value) && num(value.pushed) && num(value.failed);
}

function isRoomPage(value: unknown): value is RoomPageView {
  return (
    rec(value) &&
    str(value.title) &&
    strings(value.contributors) &&
    bool(value.hasMerge) &&
    bool(value.branched) &&
    bool(value.sharedByMe) &&
    num(value.mergeableParts) &&
    strings(value.listedOnly)
  );
}

function isNewConcept(value: unknown): value is NewConceptView {
  return rec(value) && str(value.title) && strings(value.authors);
}

export function isSharedRoomView(value: unknown): value is SharedRoomView {
  return rec(value) && list(value.pages, isRoomPage) && list(value.newConcepts, isNewConcept) && str(value.summary) && str(value.shareNotice);
}

export function isSharedMerge(value: unknown): value is SharedMerge {
  return (
    rec(value) &&
    str(value.code) &&
    bool(value.usedLlm) &&
    (value.fallbackReason === null || str(value.fallbackReason)) &&
    strings(value.contributors) &&
    strings(value.listedOnly) &&
    str(value.markdown)
  );
}

export function isSharedQuizQuestion(value: unknown): value is SharedQuizQuestion {
  return (
    rec(value) && num(value.itemId) && (value.kind === 'mcq' || value.kind === 'cloze') && str(value.prompt) && strings(value.choices) && str(value.pageName)
  );
}

export const sharedWikiCommands = {
  previewShare: (pageId: number, roomId: string) => invoke<SharePreview>('shared_wiki_preview', { pageId, roomId }),
  /** `confirmed` is true only after the user saw the confirmation screen; `allowLlmMerge` is the separate AI-merge consent. */
  sharePage: (pageId: number, roomId: string, allowLlmMerge: boolean, confirmed: boolean) =>
    invoke<SharedAction>('shared_wiki_share', { pageId, roomId, allowLlmMerge, confirmed }),
  syncShared: (roomId: string) => invoke<SharedSync>('shared_wiki_sync', { roomId }),
  withdrawShared: (pageId: number, roomId: string) => invoke<SharedAction>('shared_wiki_withdraw', { pageId, roomId }),
  sharedRoom: (roomId: string) => invoke<SharedRoomView>('shared_wiki_room', { roomId }),
  acknowledgeConcepts: (roomId: string, titles: string[]) => invoke<void>('shared_wiki_acknowledge', { roomId, titles }),
  mergeShared: (roomId: string, pageName: string) => invoke<SharedMerge>('shared_wiki_merge', { roomId, pageName }),
  startSharedQuiz: (roomId: string) => invoke<SharedQuizQuestion[]>('shared_wiki_quiz_start', { roomId }),
  answerSharedQuiz: (itemId: number, answer: string) => invoke<boolean | null>('shared_wiki_quiz_answer', { itemId, answer }),
};
