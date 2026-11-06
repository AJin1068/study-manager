import { describe, expect, it } from 'vitest';
import type { VerdictMessage } from './protocol';
import { blockedPageUrl, decideVerdict, parseBlockedParams, reasonText } from './verdict';

const getUrl = (path: string): string => `chrome-extension://id/${path}`;
const block: VerdictMessage = { type: 'verdict', tabId: 1, verdict: 'block', reasonCode: 'blocked_site' };

describe('decideVerdict', () => {
  it('redirects a block to the blocked page with domain and reason only', () => {
    const action = decideVerdict(block, 0, 'youtube.com', getUrl);
    expect(action).toEqual({ kind: 'redirect', url: 'chrome-extension://id/blocked.html?d=youtube.com&r=blocked_site' });
  });

  it('leaves allowed pages alone', () => {
    expect(decideVerdict({ ...block, verdict: 'allow', reasonCode: 'no_rules' }, 0, 'a.test', getUrl)).toEqual({ kind: 'none' });
  });

  it('ignores a stale verdict while a newer request is outstanding', () => {
    expect(decideVerdict(block, 1, 'youtube.com', getUrl)).toEqual({ kind: 'none' });
  });

  it('does nothing for an unknown tab', () => {
    expect(decideVerdict(block, 0, null, getUrl)).toEqual({ kind: 'none' });
  });
});

describe('blocked page params', () => {
  it('round-trips a domain and reason', () => {
    const url = blockedPageUrl(getUrl, 'a.example.com', 'blocked_site');
    expect(parseBlockedParams(url.slice(url.indexOf('?')))).toEqual({ domain: 'a.example.com', reasonCode: 'blocked_site' });
  });

  it('falls back on unexpected values', () => {
    expect(parseBlockedParams('?d=%3Cscript%3E&r=Bad%20Reason')).toEqual({ domain: '', reasonCode: 'unknown' });
    expect(parseBlockedParams('')).toEqual({ domain: '', reasonCode: 'unknown' });
    expect(reasonText('unknown')).toContain('공부 시간');
  });
});

describe('reasonText', () => {
  it('has strict-mode text for the reason code the core emits', () => {
    expect(reasonText('strict_unlisted')).toContain('엄격 모드');
    expect(reasonText('strict_unlisted')).not.toBe(reasonText('unknown'));
  });
});
