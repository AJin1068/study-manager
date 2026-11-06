// Wire messages of the browser link. Mirrors src-tauri/src/browser_link/protocol.rs: JSON
// objects with a snake_case `type` tag and camelCase fields.
//
// Privacy: the page title is never sent, and the builders accept only the fields below.

export const HOST_NAME = 'com.studymanager.app.browser';
export const PROTOCOL_VERSION = 1;

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

export function buildTabEvent(tabId: number, url: string, kind: TabKind): TabEventMessage {
  return { type: 'tab_event', url, tabId, kind };
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
