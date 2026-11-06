import { describe, expect, it } from 'vitest';
import { MAX_TITLE_CHARS, buildHello, buildTabEvent, parseIncoming } from './protocol';

describe('outgoing messages', () => {
  it('matches the wire format of the app', () => {
    expect(buildHello('ext', '0.1.0')).toEqual({ type: 'hello', extId: 'ext', version: '0.1.0' });
    expect(buildTabEvent(7, 'https://a.test/x', 'activated')).toEqual({
      type: 'tab_event',
      url: 'https://a.test/x',
      tabId: 7,
      kind: 'activated',
    });
  });

  it('carries a title only when one is given, shortened, and never an empty one', () => {
    expect(Object.keys(buildTabEvent(1, 'https://a.test/', 'updated')).sort()).toEqual(['kind', 'tabId', 'type', 'url']);
    expect(Object.keys(buildTabEvent(1, 'https://a.test/', 'updated', '   ')).sort()).toEqual(['kind', 'tabId', 'type', 'url']);
    expect(buildTabEvent(1, 'https://a.test/', 'activated', ' 공부 노트 ').title).toBe('공부 노트');
    expect(Array.from(buildTabEvent(1, 'https://a.test/', 'activated', '가'.repeat(500)).title ?? '')).toHaveLength(MAX_TITLE_CHARS);
  });
});

describe('parseIncoming', () => {
  it('accepts the three app messages', () => {
    expect(parseIncoming({ type: 'hello_ack', protocol: 1 })).toEqual({ type: 'hello_ack', protocol: 1 });
    expect(parseIncoming({ type: 'verdict', tabId: 3, verdict: 'block', reasonCode: 'blocked_site' })).toEqual({
      type: 'verdict',
      tabId: 3,
      verdict: 'block',
      reasonCode: 'blocked_site',
    });
    expect(parseIncoming({ type: 'error', code: 'app_not_running' })).toEqual({ type: 'error', code: 'app_not_running' });
  });

  it('rejects malformed or unknown messages', () => {
    expect(parseIncoming(null)).toBeNull();
    expect(parseIncoming('verdict')).toBeNull();
    expect(parseIncoming({ type: 'verdict', tabId: '3', verdict: 'block', reasonCode: 'x' })).toBeNull();
    expect(parseIncoming({ type: 'verdict', tabId: 3, verdict: 'maybe', reasonCode: 'x' })).toBeNull();
    expect(parseIncoming({ type: 'hello_ack' })).toBeNull();
    expect(parseIncoming({ type: 'surprise' })).toBeNull();
  });
});
