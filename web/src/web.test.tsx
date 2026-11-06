import { renderToString } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ApiError, memoryTokens, mockApi, MOCK_SLUG, WebApiClient } from './api';
import { loadConfig, parseConfig } from './config';
import { decodeDashboard, decodePublicProfile, parseRoute, summarize } from './model';
import { DashboardView, LoginForm, ProfileView } from './views';

const ENDPOINT = 'https://proj.example.test';
const KEY = 'anon-key-for-tests';
const SLUG = '0123456789abcdef';

interface Seen {
  url: string;
  method: string;
  headers: Record<string, string>;
  body: string | null;
}

/** A stand-in for PostgREST and GoTrue: public RPC for anyone, own rows only for a signed-in token. */
function fakeServer() {
  const seen: Seen[] = [];
  const fetchFn: typeof fetch = async (input, init) => {
    const url = String(input);
    const headers = (init?.headers ?? {}) as Record<string, string>;
    seen.push({ url, method: init?.method ?? 'GET', headers, body: typeof init?.body === 'string' ? init.body : null });
    const json = (status: number, value: unknown) => new Response(JSON.stringify(value), { status, headers: { 'content-type': 'application/json' } });
    const signedIn = headers.Authorization === 'Bearer user-token';
    const path = url.slice(ENDPOINT.length);
    if (path === '/rest/v1/rpc/get_public_profile') {
      const slug = (JSON.parse(String(init?.body)) as { p_slug: string }).p_slug;
      // The server returns only public sections to an anonymous caller.
      return slug === SLUG
        ? json(200, { code: 'ok', name: '민수', sections: { level: 7, grass: [{ day: '2026-11-05', study_min: 90 }] } })
        : json(200, { code: 'not_found' });
    }
    if (path === '/auth/v1/otp') return json(200, {});
    if (path === '/auth/v1/verify') {
      const body = JSON.parse(String(init?.body)) as { token: string };
      return body.token === '123456' ? json(200, { access_token: 'user-token' }) : json(400, { error: 'bad' });
    }
    if (path.startsWith('/rest/v1/daily_summaries')) {
      return signedIn ? json(200, [{ day: '2026-11-06', study_min: 60, plan_min: 120 }, { day: '2026-11-05', study_min: 30, plan_min: 60 }]) : json(401, {});
    }
    if (path.startsWith('/rest/v1/member_status')) return signedIn ? json(200, [{ level: 7 }]) : json(401, {});
    if (path.startsWith('/rest/v1/inventory_public')) return signedIn ? json(200, [{ species: 'fox' }]) : json(401, {});
    return json(404, {});
  };
  return { fetchFn, seen };
}

function client() {
  const server = fakeServer();
  const config = parseConfig({ mode: 'live', endpoint: ENDPOINT, anonKey: KEY });
  if (config?.mode !== 'live') throw new Error('config');
  return { ...server, api: new WebApiClient(config, server.fetchFn, memoryTokens()) };
}

describe('config', () => {
  it('accepts mock and secure live settings only', () => {
    expect(parseConfig({ mode: 'mock' })).toEqual({ mode: 'mock' });
    expect(parseConfig({ mode: 'live', endpoint: 'https://a.example.test/', anonKey: ' k ' })).toEqual({ mode: 'live', endpoint: 'https://a.example.test', anonKey: 'k' });
    expect(parseConfig({ mode: 'live', endpoint: 'http://localhost:54321', anonKey: 'k' })?.mode).toBe('live');
    for (const bad of [
      { mode: 'live', endpoint: 'http://a.example.test', anonKey: 'k' },
      { mode: 'live', endpoint: 'https://u:p@a.example.test', anonKey: 'k' },
      { mode: 'live', endpoint: 'https://a.example.test?x=1', anonKey: 'k' },
      { mode: 'live', endpoint: 'https://a.example.test', anonKey: '' },
      { mode: 'live', endpoint: 'nope', anonKey: 'k' },
      { mode: 'other' },
      null,
    ]) {
      expect(parseConfig(bad)).toBeNull();
    }
  });

  it('reads config.json and fails closed', async () => {
    const ok: typeof fetch = async () => new Response('{"mode":"mock"}');
    const broken: typeof fetch = async () => new Response('x', { status: 404 });
    const down: typeof fetch = async () => {
      throw new Error('offline');
    };
    expect(await loadConfig(ok)).toEqual({ mode: 'mock' });
    expect(await loadConfig(broken)).toBeNull();
    expect(await loadConfig(down)).toBeNull();
  });
});

describe('routes', () => {
  it('opens a profile only for a 16-hex link id', () => {
    expect(parseRoute(`#/u/${SLUG}`)).toEqual({ kind: 'profile', slug: SLUG });
    expect(parseRoute('#/dashboard')).toEqual({ kind: 'dashboard' });
    for (const bad of ['', '#/u/short', `#/u/${SLUG}/x`, '#/u/ZZZZZZZZZZZZZZZZ', '#/u/../../x']) expect(parseRoute(bad)).toEqual({ kind: 'home' });
  });
});

describe('public profile through the REST adapter', () => {
  it('asks anonymously with the anon key and renders only the returned sections', async () => {
    const { api, seen } = client();
    const result = await api.publicProfile(SLUG);
    expect(seen).toHaveLength(1);
    expect(seen[0]?.url).toBe(`${ENDPOINT}/rest/v1/rpc/get_public_profile`);
    expect(seen[0]?.headers.Authorization).toBe(`Bearer ${KEY}`);
    expect(seen[0]?.headers.apikey).toBe(KEY);
    const html = renderToString(<ProfileView result={result} />);
    expect(html).toContain('민수');
    expect(html).toContain('레벨 7');
    expect(html).toContain('공부 잔디');
    for (const hidden of ['배지', '지금 공부 중', '공개한 위키', '방문자', '오두막']) expect(html).not.toContain(hidden);
  });

  it('never sends the user token on the public call even when signed in', async () => {
    const { api, seen } = client();
    await api.verifyOtp('a@example.test', '123456');
    await api.publicProfile(SLUG);
    expect(seen.at(-1)?.headers.Authorization).toBe(`Bearer ${KEY}`);
  });

  it('shows an empty state for an unknown link and for a profile with nothing public', async () => {
    const { api } = client();
    expect(renderToString(<ProfileView result={await api.publicProfile('ffffffffffffffff')} />)).toContain('찾을 수 없');
    const empty = decodePublicProfile({ code: 'ok', name: '민수', sections: {} });
    expect(renderToString(<ProfileView result={empty} />)).toContain('공개된 내용이 없습니다');
  });

  it('drops unknown or malformed sections and never lists the guestbook', () => {
    const result = decodePublicProfile({
      code: 'ok',
      name: 'n',
      sections: { attempts_today: 9, window_title: 'x', guestbook: [{ body: 'secret line' }], badges: 'oops', level: 3 },
    });
    expect(result.kind === 'ok' && result.profile.shown).toEqual(['level', 'badges']);
    const html = renderToString(<ProfileView result={result} />);
    expect(html).not.toContain('secret line');
    expect(html).not.toContain('attempts');
    expect(decodePublicProfile({ code: 'not_found' })).toEqual({ kind: 'not_found' });
    expect(decodePublicProfile('garbage')).toEqual({ kind: 'not_found' });
  });

  it('turns a server failure into an error the page can show', async () => {
    const api = new WebApiClient({ mode: 'live', endpoint: ENDPOINT, anonKey: KEY }, async () => new Response('{}', { status: 500 }), memoryTokens());
    await expect(api.publicProfile(SLUG)).rejects.toBeInstanceOf(ApiError);
  });
});

describe('dashboard through the REST adapter', () => {
  it('needs a sign-in, then reads only the own aggregate rows', async () => {
    const { api, seen } = client();
    await expect(api.dashboard()).rejects.toMatchObject({ status: 401 });
    await api.requestOtp('a@example.test');
    expect(JSON.parse(seen[0]?.body ?? '{}')).toEqual({ email: 'a@example.test', create_user: false });
    await expect(api.verifyOtp('a@example.test', '000000')).rejects.toBeInstanceOf(ApiError);
    expect(api.signedIn()).toBe(false);
    await api.verifyOtp('a@example.test', '123456');
    const data = await api.dashboard();
    expect(data).toEqual({ level: 7, species: 'fox', days: [{ day: '2026-11-06', studyMin: 60, planMin: 120 }, { day: '2026-11-05', studyMin: 30, planMin: 60 }] });
    const tables = seen.filter((s) => s.url.includes('/rest/v1/') && !s.url.includes('/rpc/')).map((s) => s.url.split('/rest/v1/')[1]?.split('?')[0]);
    expect(tables.sort()).toEqual(['daily_summaries', 'inventory_public', 'member_status']);
    expect(seen.filter((s) => s.url.includes('/rest/v1/')).every((s) => s.headers.Authorization === 'Bearer user-token')).toBe(true);
  });

  it('signs out when the server rejects the token', async () => {
    const tokens = memoryTokens();
    tokens.set('expired');
    const api = new WebApiClient({ mode: 'live', endpoint: ENDPOINT, anonKey: KEY }, async () => new Response('{}', { status: 401 }), tokens);
    await expect(api.dashboard()).rejects.toMatchObject({ status: 401 });
    expect(api.signedIn()).toBe(false);
  });

  it('summarizes the last seven days and renders no app, title or attempt field', () => {
    const data = decodeDashboard(
      [
        { day: '2026-11-06', study_min: 60, plan_min: 120 },
        { day: '2026-11-05', study_min: 30, plan_min: 60 },
        { day: '2026-10-01', study_min: 500, plan_min: 10 },
        { day: 'bad', study_min: 'x', plan_min: 1 },
      ],
      [{ level: 7 }],
      [{ species: 'fox' }],
    );
    const summary = summarize(data.days, '2026-11-06');
    expect(summary.weekStudyMin).toBe(90);
    expect(summary.weekAchievementPct).toBe(50);
    expect(summary.rows.map((r) => r.day)).toEqual(['2026-11-06', '2026-11-05', '2026-10-01']);
    expect(summarize([], '2026-11-06').weekAchievementPct).toBeNull();
    expect(summarize([{ day: '2026-11-06', studyMin: 500, planMin: 10 }], '2026-11-06').weekAchievementPct).toBe(100);
    const html = renderToString(<DashboardView data={data} summary={summary} />);
    expect(html).toContain('달성률 50%');
    expect(html).toContain('레벨 7');
    expect(html.toLowerCase()).not.toMatch(/attempt|딴짓|window|앱 이름|app_name/);
  });

  it('renders the login steps and an error', () => {
    const base = { busy: false, error: null, onEmail: () => undefined, onCode: () => undefined };
    expect(renderToString(<LoginForm {...base} step="email" />)).toContain('type="email"');
    expect(renderToString(<LoginForm {...base} step="code" error="실패" />)).toContain('실패');
  });
});

describe('mock mode', () => {
  it('serves fixed data through the same interface', async () => {
    const api = mockApi('2026-11-06');
    const found = await api.publicProfile(MOCK_SLUG);
    expect(found.kind === 'ok' && found.profile.shown).toEqual(['grass', 'level', 'badges']);
    expect(await api.publicProfile('ffffffffffffffff')).toEqual({ kind: 'not_found' });
    await expect(api.dashboard()).rejects.toMatchObject({ status: 401 });
    await api.verifyOtp('a@example.test', '1');
    expect((await api.dashboard()).days).toHaveLength(5);
  });
});
