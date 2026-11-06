import type { ChromeLike, NativePort, NavigationDetails, TabInfo } from './chromeApi';

type Callback = (...args: never[]) => void;

class FakeListener<F extends Callback> {
  private callbacks: F[] = [];
  addListener(callback: F): void {
    this.callbacks.push(callback);
  }
  emit(...args: Parameters<F>): void {
    for (const callback of this.callbacks) (callback as (...a: Parameters<F>) => void)(...args);
  }
}

export class FakePort implements NativePort {
  sent: unknown[] = [];
  onMessage = new FakeListener<(message: unknown) => void>();
  onDisconnect = new FakeListener<() => void>();
  postMessage(message: unknown): void {
    this.sent.push(message);
  }
  disconnect(): void {
    this.onDisconnect.emit();
  }
}

export interface FakeChrome {
  api: ChromeLike;
  ports: FakePort[];
  tabs: Map<number, TabInfo>;
  updates: { tabId: number; url: string }[];
  badges: string[];
  activated: FakeListener<(info: { tabId: number }) => void>;
  beforeNavigate: FakeListener<(details: NavigationDetails) => void>;
  historyState: FakeListener<(details: NavigationDetails) => void>;
}

export function createFakeChrome(): FakeChrome {
  const ports: FakePort[] = [];
  const tabs = new Map<number, TabInfo>();
  const updates: { tabId: number; url: string }[] = [];
  const badges: string[] = [];
  const activated = new FakeListener<(info: { tabId: number }) => void>();
  const beforeNavigate = new FakeListener<(details: NavigationDetails) => void>();
  const historyState = new FakeListener<(details: NavigationDetails) => void>();
  const api: ChromeLike = {
    runtime: {
      id: 'abcdefghijklmnopabcdefghijklmnop',
      connectNative: () => {
        const port = new FakePort();
        ports.push(port);
        return port;
      },
      getManifest: () => ({ version: '0.1.0' }),
      getURL: (path) => `chrome-extension://abcdefghijklmnopabcdefghijklmnop/${path}`,
      onStartup: new FakeListener<() => void>(),
      onInstalled: new FakeListener<() => void>(),
    },
    tabs: {
      get: (tabId) => {
        const tab = tabs.get(tabId);
        return tab ? Promise.resolve(tab) : Promise.reject(new Error('no tab'));
      },
      update: (tabId, properties) => {
        updates.push({ tabId, url: properties.url });
        return Promise.resolve({});
      },
      onActivated: activated,
    },
    webNavigation: { onBeforeNavigate: beforeNavigate, onHistoryStateUpdated: historyState },
    action: {
      setBadgeText: ({ text }) => {
        badges.push(text);
      },
      setBadgeBackgroundColor: () => undefined,
    },
  };
  return { api, ports, tabs, updates, badges, activated, beforeNavigate, historyState };
}
