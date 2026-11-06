// Mirror of src-tauri/src/commands/messages.rs and the `message://cheer` event.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { UpdateOutcome } from './settings';

export type MessageTone = 'gentle' | 'warm' | 'normal' | 'firm' | 'harsh';
export type MessageKind = 'distraction' | 'streak' | 'set_end' | 'random';
export type MessageSource = 'template' | 'wiki' | 'llm';

export type MessageSettingKey =
  | 'enabled'
  | 'tone'
  | 'maxPerHour'
  | 'eventsEnabled'
  | 'randomEnabled'
  | 'randomIntervalMin'
  | 'notifyEnabled';

export interface MessageConfig {
  enabled: boolean;
  tone: MessageTone;
  maxPerHour: number;
  eventsEnabled: boolean;
  randomEnabled: boolean;
  randomIntervalMin: number;
  notifyEnabled: boolean;
}

/** What the window shows. The system notification text is not part of it. */
export interface CheerMessage {
  kind: MessageKind;
  tone: MessageTone;
  source: MessageSource;
  text: string;
}

const TONES: readonly string[] = ['gentle', 'warm', 'normal', 'firm', 'harsh'];
const KINDS: readonly string[] = ['distraction', 'streak', 'set_end', 'random'];
const SOURCES: readonly string[] = ['template', 'wiki', 'llm'];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

export function isCheerMessage(value: unknown): value is CheerMessage {
  return (
    isRecord(value) &&
    typeof value.kind === 'string' &&
    KINDS.includes(value.kind) &&
    typeof value.tone === 'string' &&
    TONES.includes(value.tone) &&
    typeof value.source === 'string' &&
    SOURCES.includes(value.source) &&
    typeof value.text === 'string' &&
    value.text.length > 0
  );
}

export const messageCommands = {
  getMessageSettings: () => invoke<MessageConfig>('get_message_settings'),
  updateMessageSetting: (key: MessageSettingKey, value: number | boolean | string) =>
    invoke<UpdateOutcome>('update_message_setting', { key, value }),
};

/** A cheer or scold message for a toast; only sent while a study block runs. */
export function onCheerMessage(handler: (message: CheerMessage) => void): Promise<UnlistenFn> {
  return listen<unknown>('message://cheer', (event) => {
    if (isCheerMessage(event.payload)) handler(event.payload);
  });
}
