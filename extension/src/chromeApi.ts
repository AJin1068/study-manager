// The slice of the Chrome extension API this package uses. Declared here instead of pulling in
// @types/chrome so the controller can be driven by a fake in tests.

export interface Listener<F> {
  addListener(callback: F): void;
}

export interface NativePort {
  postMessage(message: unknown): void;
  disconnect(): void;
  onMessage: Listener<(message: unknown) => void>;
  onDisconnect: Listener<() => void>;
}

export interface TabInfo {
  id?: number;
  url?: string;
}

export interface NavigationDetails {
  tabId: number;
  frameId: number;
  url: string;
}

export interface ChromeLike {
  runtime: {
    id: string;
    lastError?: { message?: string } | undefined;
    connectNative(hostName: string): NativePort;
    getManifest(): { version: string };
    getURL(path: string): string;
    onStartup: Listener<() => void>;
    onInstalled: Listener<() => void>;
  };
  tabs: {
    get(tabId: number): Promise<TabInfo>;
    update(tabId: number, properties: { url: string }): Promise<unknown>;
    onActivated: Listener<(info: { tabId: number }) => void>;
  };
  webNavigation: {
    onBeforeNavigate: Listener<(details: NavigationDetails) => void>;
    onHistoryStateUpdated: Listener<(details: NavigationDetails) => void>;
  };
  action: {
    setBadgeText(details: { text: string }): Promise<void> | void;
    setBadgeBackgroundColor(details: { color: string }): Promise<void> | void;
  };
}
