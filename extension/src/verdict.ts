import type { VerdictMessage } from './protocol';

export const BLOCKED_PAGE = 'blocked.html';
const REASON_PATTERN = /^[a-z0-9_]{1,40}$/;
const DOMAIN_PATTERN = /^[a-z0-9.-]{1,253}$/;

export type VerdictAction = { kind: 'none' } | { kind: 'redirect'; url: string };

/**
 * What to do with a verdict. `outstanding` is how many requests for the tab are still waiting
 * after this one: a verdict for a page the user already left must not redirect the new page.
 * `domain` is null when the tab is unknown, which also means no action.
 */
export function decideVerdict(
  verdict: VerdictMessage,
  outstanding: number,
  domain: string | null,
  getUrl: (path: string) => string,
): VerdictAction {
  if (verdict.verdict !== 'block' || outstanding > 0 || domain === null) return { kind: 'none' };
  return { kind: 'redirect', url: blockedPageUrl(getUrl, domain, verdict.reasonCode) };
}

/** Domain and reason code only; the blocked page never learns the path or query. */
export function blockedPageUrl(getUrl: (path: string) => string, domain: string, reasonCode: string): string {
  const query = new URLSearchParams({ d: domain, r: reasonCode });
  return `${getUrl(BLOCKED_PAGE)}?${query.toString()}`;
}

export interface BlockedPageInfo {
  domain: string;
  reasonCode: string;
}

/** Reads the blocked page query defensively; unexpected values fall back to neutral text. */
export function parseBlockedParams(search: string): BlockedPageInfo {
  const params = new URLSearchParams(search);
  const domain = params.get('d') ?? '';
  const reason = params.get('r') ?? '';
  return {
    domain: DOMAIN_PATTERN.test(domain) ? domain : '',
    reasonCode: REASON_PATTERN.test(reason) ? reason : 'unknown',
  };
}

const REASON_TEXT: Record<string, string> = {
  blocked_site: '공부 시간에는 열 수 없는 사이트예요.',
  strict_unlisted: '엄격 모드에서는 허용 목록에 없는 사이트를 열 수 없어요.',
};

export function reasonText(reasonCode: string): string {
  return REASON_TEXT[reasonCode] ?? '지금은 공부 시간이라 이 사이트를 열 수 없어요.';
}
