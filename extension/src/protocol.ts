// Wire messages of the browser link. Mirrors src-tauri/src/browser_link/protocol.rs: JSON
// objects with a snake_case `type` tag and camelCase fields.
//
// Privacy: the builders accept only the fields below. The page title is sent only for a tab the
// user just switched to, shortened to MAX_TITLE_CHARS; the app uses it in memory for the optional
// LLM tab judge (off by default) and never stores or logs it.

export const HOST_NAME = 'com.studymanager.app.browser';
export const PROTOCOL_VERSION = 1;
/** Matches MAX_TAB_LABEL_CHARS in src-tauri/src/llm/egress.rs. */
export const MAX_TITLE_CHARS = 120;

export type TabKind = 'activated' | 'updated';
export type VerdictKind = 'allow' | 'block';

export interface HelloMessage {
  type: 'hello';
  extId: string;
  version: string;
}

export interface TabEventMessage {
  type: 'tab_event';
  url: string;
  tabId: number;
  kind: TabKind;
  title?: string;
}

export type OutgoingMessage = HelloMessage | TabEventMessage;

export interface HelloAck {
  type: 'hello_ack';
  protocol: number;
}

export interface VerdictMessage {
  type: 'verdict';
  tabId: number;
  verdict: VerdictKind;
  reasonCode: string;
}

export interface ErrorMessage {
  type: 'error';
  code: string;
}

export type IncomingMessage = HelloAck | VerdictMessage | ErrorMessage;

export function buildHello(extId: string, version: string): HelloMessage {
  return { type: 'hello', extId, version };
}

export function buildTabEvent(tabId: number, url: string, kind: TabKind, title?: string): TabEventMessage {
  const message: TabEventMessage = { type: 'tab_event', url, tabId, kind };
  const short = title === undefined ? '' : Array.from(title.trim()).slice(0, MAX_TITLE_CHARS).join('');
  return short === '' ? message : { ...message, title: short };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

/** Validates a message from the host; anything unexpected is `null` and gets ignored. */
export function parseIncoming(raw: unknown): IncomingMessage | null {
  if (!isRecord(raw)) return null;
  switch (raw.type) {
    case 'hello_ack':
      return typeof raw.protocol === 'number' ? { type: 'hello_ack', protocol: raw.protocol } : null;
    case 'verdict':
      if (
        typeof raw.tabId === 'number' &&
        (raw.verdict === 'allow' || raw.verdict === 'block') &&
        typeof raw.reasonCode === 'string'
      ) {
        return { type: 'verdict', tabId: raw.tabId, verdict: raw.verdict, reasonCode: raw.reasonCode };
      }
      return null;
    case 'error':
      return typeof raw.code === 'string' ? { type: 'error', code: raw.code } : null;
    default:
      return null;
  }
}
