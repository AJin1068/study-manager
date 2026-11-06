import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { ChannelError, isSafeNtfyServer, makeSender, RecordingChannel, type FetchLike } from './channels.ts';
import { TEST_MESSAGE } from './detect.ts';

const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const contract = JSON.parse(readFileSync(join(root, 'fixtures', 'notify', 'contract.json'), 'utf8'));

function recorder(status = 200) {
  const calls: { url: string; method: string; headers: Record<string, string>; body: string }[] = [];
  const fetchFn: FetchLike = async (url, init) => {
    calls.push({ url, ...init });
    return { ok: status >= 200 && status < 300, status };
  };
  return { calls, fetchFn };
}

describe('shared notify contract', () => {
  it('uses the same test text as the desktop client', () => {
    expect(TEST_MESSAGE).toBe(contract.messages.test);
  });

  for (const c of contract.ntfy.requests) {
    it(`ntfy: ${c.name}`, async () => {
      const { calls, fetchFn } = recorder();
      await makeSender(fetchFn, undefined).send({ channel: 'ntfy', server: c.server, topic: c.topic }, c.message);
      expect(calls).toHaveLength(1);
      expect(calls[0].method).toBe(c.expect.method);
      expect(calls[0].url).toBe(c.expect.url);
      expect(calls[0].body).toBe(c.expect.body);
      expect(calls[0].headers['Content-Type']).toBe(c.expect.contentType);
      for (const [name, value] of Object.entries(c.expect.headers)) expect(calls[0].headers[name[0].toUpperCase() + name.slice(1)]).toBe(value);
    });
  }

  for (const c of contract.telegram.requests) {
    it(`telegram: ${c.name}`, async () => {
      const { calls, fetchFn } = recorder();
      await makeSender(fetchFn, c.token).send({ channel: 'telegram', chatId: c.chatId }, c.message);
      expect(calls).toHaveLength(1);
      expect(calls[0].method).toBe(c.expect.method);
      expect(calls[0].url).toBe(c.expect.url);
      expect(calls[0].headers['Content-Type']).toBe(c.expect.contentType);
      expect(JSON.parse(calls[0].body)).toEqual(c.expect.body);
    });
  }

  it('declares the server-side test route the desktop calls', () => {
    expect(contract.serverTest.route).toBe('/functions/v1/missed-plan');
    expect(contract.serverTest.requestBody).toEqual({ action: 'test' });
  });
});

describe('ntfy server safety', () => {
  for (const bad of contract.unsafeNtfyServers) {
    it(`refuses ${bad}`, async () => {
      expect(isSafeNtfyServer(bad)).toBe(false);
      const { calls, fetchFn } = recorder();
      await expect(makeSender(fetchFn, undefined).send({ channel: 'ntfy', server: bad, topic: 't' }, 'x')).rejects.toBeInstanceOf(ChannelError);
      expect(calls).toHaveLength(0);
    });
  }
  for (const good of contract.safeNtfyServers) {
    it(`accepts ${good}`, () => expect(isSafeNtfyServer(good)).toBe(true));
  }
});

describe('ntfy server SSRF matrix', () => {
  const refuse = [
    'https://localhost', 'https://localhost.', 'https://LOCALHOST.', 'https://metadata.google.internal.', 'https://foo.internal.',
    'https://foo.local.', 'https://x.localhost.', 'https://127.0.0.1', 'https://127.1', 'https://2130706433', 'https://0x7f.1',
    'https://169.254.169.254.', 'https://[::1]', 'https://ntfy.sh.', 'https://ntfy.sh:8443', 'https://u:p@ntfy.sh', 'https://ntfy.sh/path',
    'https://ntfy.sh/?q=1', 'http://ntfy.sh', 'https://ntfy', 'ftp://ntfy.sh', 'https://ntfy.sh#x',
  ];
  for (const bad of refuse) {
    it(`refuses ${bad}`, () => expect(isSafeNtfyServer(bad)).toBe(false));
  }
});

describe('failures', () => {
  it('reports an HTTP failure as a code without the URL', async () => {
    const { fetchFn } = recorder(403);
    const err = await makeSender(fetchFn, 'tok').send({ channel: 'telegram', chatId: '1' }, 'x').catch((e) => e);
    expect(err).toBeInstanceOf(ChannelError);
    expect(err.message).toBe('http_403');
    expect(err.message).not.toContain('tok');
  });
  it('telegram without the function secret fails closed', async () => {
    const { calls, fetchFn } = recorder();
    await expect(makeSender(fetchFn, undefined).send({ channel: 'telegram', chatId: '1' }, 'x')).rejects.toThrow('telegram_not_configured');
    expect(calls).toHaveLength(0);
  });
  it('the recording channel keeps messages and can be told to fail', async () => {
    const rec = new RecordingChannel();
    rec.failNext = 1;
    await expect(rec.send({ channel: 'telegram', chatId: '1' }, 'a')).rejects.toThrow();
    await rec.send({ channel: 'telegram', chatId: '1' }, 'b');
    expect(rec.sent.map((s) => s.text)).toEqual(['b']);
  });
});
