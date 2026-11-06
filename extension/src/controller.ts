import type { ChromeLike, NativePort } from './chromeApi';
import { EventFilter, domainOf, isWebUrl } from './events';
import {
  HOST_NAME,
  buildHello,
  buildTabEvent,
  parseIncoming,
  type TabKind,
  type VerdictMessage,
} from './protocol';
import { decideVerdict } from './verdict';

export type LinkState = 'disconnected' | 'connecting' | 'connected';

const MAX_QUEUED = 16;
const BADGE_OFF = { text: '끊김', color: '#b45309' };
const BADGE_ON = { text: '', color: '#15803d' };

interface QueuedEvent {
  tabId: number;
  url: string;
  kind: TabKind;
  title?: string;
}

/**
 * Connects the browser to the app and turns verdicts into redirects.
 *
 * Messages leave only from browser events (tab switch, address change); there is no timer. While
 * the link is not established the page is left alone (fail-open) and the badge shows it. A lost
 * link is re-established lazily by the next event, so there is no retry loop either.
 */
export class LinkController {
  private port: NativePort | null = null;
  private state: LinkState = 'disconnected';
  private queue: QueuedEvent[] = [];
  /** tabId -> requests sent and not yet answered, and the domain of the latest one. */
  private pending = new Map<number, { count: number; domain: string | null }>();
  private readonly filter: EventFilter;

  constructor(
    private readonly chrome: ChromeLike,
    now: () => number = Date.now,
  ) {
    this.filter = new EventFilter(now);
  }

  get linkState(): LinkState {
    return this.state;
  }

  /** Registers the browser listeners. Must run synchronously at service worker start. */
  register(): void {
    const { tabs, webNavigation, runtime } = this.chrome;
    tabs.onActivated.addListener(({ tabId }) => {
      void this.onActivated(tabId);
    });
    webNavigation.onBeforeNavigate.addListener((d) => {
      if (d.frameId === 0) this.handleEvent(d.tabId, d.url, 'updated');
    });
    webNavigation.onHistoryStateUpdated.addListener((d) => {
      if (d.frameId === 0) this.handleEvent(d.tabId, d.url, 'updated');
    });
    runtime.onStartup.addListener(() => this.ensureConnected());
    runtime.onInstalled.addListener(() => this.ensureConnected());
    this.ensureConnected();
  }

  private async onActivated(tabId: number): Promise<void> {
    try {
      const tab = await this.chrome.tabs.get(tabId);
      this.handleEvent(tabId, tab.url, 'activated', tab.title);
    } catch {
      // The tab closed before it could be read: nothing to judge.
    }
  }

  handleEvent(tabId: number, url: string | undefined, kind: TabKind, title?: string): void {
    if (!isWebUrl(url) || tabId < 0) return;
    if (!this.filter.accept(tabId, url, kind)) return;
    this.ensureConnected();
    if (this.state === 'connected') {
      this.send({ tabId, url, kind, ...(title === undefined ? {} : { title }) });
    } else if (this.state === 'connecting') {
      if (this.queue.length >= MAX_QUEUED) this.queue.shift();
      this.queue.push({ tabId, url, kind, ...(title === undefined ? {} : { title }) });
    }
  }

  private ensureConnected(): void {
    if (this.state !== 'disconnected') return;
    this.state = 'connecting';
    let port: NativePort;
    try {
      port = this.chrome.runtime.connectNative(HOST_NAME);
    } catch {
      this.onDisconnected();
      return;
    }
    this.port = port;
    port.onMessage.addListener((message) => {
      if (this.port === port) this.onMessage(message);
    });
    port.onDisconnect.addListener(() => {
      // Reading lastError marks it handled; the content is not needed.
      void this.chrome.runtime.lastError;
      if (this.port === port) this.onDisconnected();
    });
    port.postMessage(buildHello(this.chrome.runtime.id, this.chrome.runtime.getManifest().version));
  }

  private onMessage(raw: unknown): void {
    const message = parseIncoming(raw);
    if (message === null) return;
    switch (message.type) {
      case 'hello_ack':
        this.state = 'connected';
        this.setBadge(BADGE_ON);
        for (const event of this.queue.splice(0)) this.send(event);
        break;
      case 'verdict':
        this.onVerdict(message);
        break;
      case 'error':
        // The host or app closes the port after an error; the disconnect handler shows it.
        break;
    }
  }

  private send(event: QueuedEvent): void {
    if (this.port === null) return;
    this.port.postMessage(buildTabEvent(event.tabId, event.url, event.kind, event.title));
    const entry = this.pending.get(event.tabId);
    this.pending.set(event.tabId, { count: (entry?.count ?? 0) + 1, domain: domainOf(event.url) });
  }

  private onVerdict(verdict: VerdictMessage): void {
    const entry = this.pending.get(verdict.tabId);
    if (!entry) return;
    const outstanding = entry.count - 1;
    if (outstanding > 0) this.pending.set(verdict.tabId, { ...entry, count: outstanding });
    else this.pending.delete(verdict.tabId);
    const action = decideVerdict(verdict, outstanding, entry.domain, (path) => this.chrome.runtime.getURL(path));
    if (action.kind === 'redirect') {
      this.chrome.tabs.update(verdict.tabId, { url: action.url }).catch(() => {
        // The tab is gone; nothing left to redirect.
      });
    }
  }

  private onDisconnected(): void {
    this.state = 'disconnected';
    this.port = null;
    this.queue = [];
    this.pending.clear();
    this.setBadge(BADGE_OFF);
  }

  private setBadge(badge: { text: string; color: string }): void {
    void this.chrome.action.setBadgeText({ text: badge.text });
    void this.chrome.action.setBadgeBackgroundColor({ color: badge.color });
  }
}
