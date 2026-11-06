// Mirror of src-tauri/src/commands/chat.rs. Types match the Rust serde output and are checked
// against src/api/fixtures/chat-contract.json by both test suites.
// The conversation lives in the UI only; the core stores drafts (plan operations), never chat text.
// A draft changes nothing until `applyChatDraft`, which goes through the study lock.

import { invoke } from '@tauri-apps/api/core';

export type ChatSpeaker = 'user' | 'assistant';

export interface ChatMessage {
  role: ChatSpeaker;
  text: string;
}

export type DraftKind = 'create' | 'move' | 'delete';

export interface DraftItem {
  kind: DraftKind;
  label: string;
}

export interface ChatDraft {
  id: number;
  items: DraftItem[];
}

export interface ChatReply {
  text: string;
  /** `local` when the model did not answer (see `fallbackReason`). */
  source: 'llm' | 'local';
  fallbackReason: string | null;
  draft: ChatDraft | null;
  /** A move or cancel was refused because a study run holds the lock. */
  locked: boolean;
  /** The model asked more questions than allowed without drafting. */
  overAsked: boolean;
}

export interface ChatApplyResult {
  applied: boolean;
  /** The study lock refused it; the attempt is in the plan history. */
  locked: boolean;
  changed: number;
  batchId: string | null;
}

const KINDS: readonly string[] = ['create', 'move', 'delete'];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

export function isChatDraft(value: unknown): value is ChatDraft {
  return (
    isRecord(value) &&
    typeof value.id === 'number' &&
    Array.isArray(value.items) &&
    value.items.every((i: unknown) => isRecord(i) && typeof i.kind === 'string' && KINDS.includes(i.kind) && typeof i.label === 'string')
  );
}

export function isChatReply(value: unknown): value is ChatReply {
  return (
    isRecord(value) &&
    typeof value.text === 'string' &&
    (value.source === 'llm' || value.source === 'local') &&
    (value.fallbackReason === null || typeof value.fallbackReason === 'string') &&
    (value.draft === null || isChatDraft(value.draft)) &&
    typeof value.locked === 'boolean' &&
    typeof value.overAsked === 'boolean'
  );
}

export function isChatApplyResult(value: unknown): value is ChatApplyResult {
  return (
    isRecord(value) &&
    typeof value.applied === 'boolean' &&
    typeof value.locked === 'boolean' &&
    typeof value.changed === 'number' &&
    (value.batchId === null || typeof value.batchId === 'string')
  );
}

export const chatCommands = {
  /** `history` is the conversation so far; it is sent along each time and never stored by the core. */
  sendChatMessage: (history: ChatMessage[], message: string) => invoke<ChatReply>('send_chat_message', { history, message }),
  applyChatDraft: (draftId: number) => invoke<ChatApplyResult>('apply_chat_draft', { draftId }),
  discardChatDraft: (draftId: number) => invoke<boolean>('discard_chat_draft', { draftId }),
  getChatDraft: (draftId: number) => invoke<ChatDraft | null>('get_chat_draft', { draftId }),
  /** Opens the small chat window (the tray's "챗봇 입력"). */
  openChatInput: () => invoke<void>('open_chat_input'),
};
