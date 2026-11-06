import type { TabKind } from './protocol';

/** Only http(s) pages are judged; extension, browser-internal, and file pages never leave the browser. */
export function isWebUrl(url: string | undefined): url is string {
  if (url === undefined) return false;
  try {
    const { protocol } = new URL(url);
    return protocol === 'http:' || protocol === 'https:';
  } catch {
    return false;
  }
}

/** The lowercase host of a web URL, or `null`. The only part of a URL kept after sending. */
export function domainOf(url: string): string | null {
  if (!isWebUrl(url)) return null;
  const host = new URL(url).hostname.toLowerCase();
  return host === '' ? null : host;
}

const DEFAULT_WINDOW_MS = 1000;
const MAX_TRACKED_TABS = 256;

/**
 * Drops repeats of the same (tab, URL, kind) inside a short window. One navigation commonly
 * raises several browser events; this is what keeps it to one message. Time based, so no timer
 * is needed (a service worker may be stopped at any moment).
 */
export class EventFilter {
  private last = new Map<number, { url: string; kind: TabKind; at: number }>();

  constructor(
    private readonly now: () => number,
    private readonly windowMs: number = DEFAULT_WINDOW_MS,
  ) {}

  accept(tabId: number, url: string, kind: TabKind): boolean {
    if (!isWebUrl(url)) return false;
    const at = this.now();
    const previous = this.last.get(tabId);
    if (previous && previous.url === url && previous.kind === kind && at - previous.at < this.windowMs) {
      return false;
    }
    if (this.last.size >= MAX_TRACKED_TABS && !this.last.has(tabId)) this.last.clear();
    this.last.set(tabId, { url, kind, at });
    return true;
  }
}
