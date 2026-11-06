import { describe, expect, it } from 'vitest';
import { EventFilter, domainOf, isWebUrl } from './events';

describe('isWebUrl / domainOf', () => {
  it('judges only http(s) pages', () => {
    expect(isWebUrl('https://example.com/a?b=1')).toBe(true);
    expect(isWebUrl('http://example.com')).toBe(true);
    for (const url of ['chrome://settings', 'chrome-extension://x/blocked.html', 'about:blank', 'file:///etc/hosts', 'nope', '', undefined]) {
      expect(isWebUrl(url)).toBe(false);
    }
  });

  it('keeps only the lowercase host', () => {
    expect(domainOf('https://User:pw@WWW.Example.COM:8443/path?q=secret#frag')).toBe('www.example.com');
    expect(domainOf('chrome://settings')).toBeNull();
  });
});

describe('EventFilter', () => {
  it('drops an identical event inside the window and passes it after', () => {
    let now = 1000;
    const filter = new EventFilter(() => now, 500);
    expect(filter.accept(1, 'https://a.test/', 'updated')).toBe(true);
    now += 100;
    expect(filter.accept(1, 'https://a.test/', 'updated')).toBe(false);
    now += 500;
    expect(filter.accept(1, 'https://a.test/', 'updated')).toBe(true);
  });

  it('treats a different tab, url, or kind as a new event', () => {
    const filter = new EventFilter(() => 0, 500);
    expect(filter.accept(1, 'https://a.test/', 'updated')).toBe(true);
    expect(filter.accept(2, 'https://a.test/', 'updated')).toBe(true);
    expect(filter.accept(1, 'https://b.test/', 'updated')).toBe(true);
    expect(filter.accept(1, 'https://b.test/', 'activated')).toBe(true);
  });

  it('rejects non-web urls', () => {
    const filter = new EventFilter(() => 0);
    expect(filter.accept(1, 'chrome://newtab', 'activated')).toBe(false);
  });
});
