import { describe, expect, it } from 'vitest';
import { LinkController } from './controller';
import { createFakeChrome } from './fakeChrome';
import { HOST_NAME } from './protocol';

function setup() {
  const fake = createFakeChrome();
  let now = 0;
  const controller = new LinkController(fake.api, () => now);
  controller.register();
  const port = fake.ports[0];
  if (!port) throw new Error('register must open the native port');
  return {
    fake,
    controller,
    port,
    advance: (ms: number) => {
      now += ms;
    },
    ack: () => port.onMessage.emit({ type: 'hello_ack', protocol: 1 }),
    nav: (tabId: number, url: string, frameId = 0) => fake.beforeNavigate.emit({ tabId, frameId, url }),
  };
}

describe('LinkController', () => {
  it('says hello on connect and sends nothing else until acknowledged', () => {
    const { port, nav, ack, controller } = setup();
    expect(port.sent).toEqual([{ type: 'hello', extId: 'abcdefghijklmnopabcdefghijklmnop', version: '0.1.0' }]);
    nav(1, 'https://a.test/x');
    expect(port.sent).toHaveLength(1);
    expect(controller.linkState).toBe('connecting');
    ack();
    expect(port.sent[1]).toEqual({ type: 'tab_event', url: 'https://a.test/x', tabId: 1, kind: 'updated' });
    expect(controller.linkState).toBe('connected');
  });

  it('sends exactly one message per event, and none without events', () => {
    const { port, nav, ack, advance, fake } = setup();
    ack();
    nav(1, 'https://a.test/');
    nav(1, 'https://a.test/'); // duplicate raised by the same navigation
    fake.historyState.emit({ tabId: 1, frameId: 0, url: 'https://a.test/' }); // same URL and kind, same moment
    advance(60_000); // time passing alone sends nothing
    expect(port.sent.filter((m) => (m as { type: string }).type === 'tab_event')).toHaveLength(1);
    nav(1, 'https://b.test/');
    expect(port.sent.filter((m) => (m as { type: string }).type === 'tab_event')).toHaveLength(2);
  });

  it('ignores sub frames and non-web pages', () => {
    const { port, nav, ack } = setup();
    ack();
    nav(1, 'https://a.test/', 3);
    nav(1, 'chrome://settings');
    nav(1, 'chrome-extension://abcdefghijklmnopabcdefghijklmnop/blocked.html?d=a.test&r=x');
    nav(-1, 'https://a.test/');
    expect(port.sent).toHaveLength(1);
  });

  it('reads the url of an activated tab and sends kind activated', async () => {
    const { port, ack, fake } = setup();
    ack();
    fake.tabs.set(5, { id: 5, url: 'https://c.test/p?q=1' });
    fake.activated.emit({ tabId: 5 });
    await Promise.resolve();
    await Promise.resolve();
    expect(port.sent[1]).toEqual({ type: 'tab_event', url: 'https://c.test/p?q=1', tabId: 5, kind: 'activated' });
    fake.activated.emit({ tabId: 99 }); // closed tab: no throw, no message
    await Promise.resolve();
    await Promise.resolve();
    expect(port.sent).toHaveLength(2);
  });

  it('redirects the tab to the blocked page on a block verdict', () => {
    const { port, nav, ack, fake } = setup();
    ack();
    nav(2, 'https://www.YouTube.com/watch?v=secret');
    port.onMessage.emit({ type: 'verdict', tabId: 2, verdict: 'block', reasonCode: 'blocked_site' });
    expect(fake.updates).toEqual([
      { tabId: 2, url: 'chrome-extension://abcdefghijklmnopabcdefghijklmnop/blocked.html?d=www.youtube.com&r=blocked_site' },
    ]);
    expect(fake.updates[0]?.url).not.toContain('secret');
  });

  it('does not redirect on allow, and drops a stale block when the user moved on', () => {
    const { port, nav, ack, fake } = setup();
    ack();
    nav(2, 'https://a.test/');
    port.onMessage.emit({ type: 'verdict', tabId: 2, verdict: 'allow', reasonCode: 'no_rules' });
    nav(2, 'https://youtube.com/');
    nav(2, 'https://docs.test/');
    port.onMessage.emit({ type: 'verdict', tabId: 2, verdict: 'block', reasonCode: 'blocked_site' }); // answers youtube
    expect(fake.updates).toEqual([]);
    port.onMessage.emit({ type: 'verdict', tabId: 2, verdict: 'allow', reasonCode: 'no_rules' });
    port.onMessage.emit({ type: 'verdict', tabId: 9, verdict: 'block', reasonCode: 'blocked_site' }); // unsolicited
    expect(fake.updates).toEqual([]);
  });

  it('fails open when the app is not running and shows the badge', () => {
    const { port, nav, ack, fake, controller } = setup();
    port.onMessage.emit({ type: 'error', code: 'app_not_running' });
    port.disconnect();
    expect(controller.linkState).toBe('disconnected');
    expect(fake.badges.at(-1)).toBe('끊김');
    expect(fake.updates).toEqual([]);
    ack(); // late frames from the dead port change nothing about redirects
    nav(1, 'https://youtube.com/'); // reconnects lazily; still nothing blocks without a verdict
    expect(fake.ports).toHaveLength(2);
    expect(fake.updates).toEqual([]);
  });

  it('reconnects on the next event and delivers the queued event after the ack', () => {
    const { port, nav, fake } = setup();
    port.disconnect();
    nav(1, 'https://a.test/');
    const second = fake.ports[1];
    if (!second) throw new Error('expected a reconnect');
    expect(second.sent).toHaveLength(1); // hello only
    second.onMessage.emit({ type: 'hello_ack', protocol: 1 });
    expect(second.sent[1]).toEqual({ type: 'tab_event', url: 'https://a.test/', tabId: 1, kind: 'updated' });
    expect(fake.badges.at(-1)).toBe('');
  });

  it('drops queued events when the link fails before the ack', () => {
    const { port, nav, fake } = setup();
    nav(1, 'https://a.test/');
    port.disconnect();
    expect(fake.ports).toHaveLength(1);
    expect(port.sent).toHaveLength(1);
  });

  it('uses the host name from the app', () => {
    expect(HOST_NAME).toBe('com.studymanager.app.browser');
  });
});
