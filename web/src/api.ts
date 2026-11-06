// The web adapter: PostgREST and GoTrue over fetch with the anon key. Row level security does the
// authorization; nothing here can read another person's private rows. A mock adapter with fixed
// data implements the same interface for development and tests.

import type { WebConfig } from './config';
import { decodeDashboard, decodePublicProfile, type DashboardData, type ProfileResult } from './model';

export class ApiError extends Error {
  constructor(readonly status: number) {
    super(`http_${status}`);
  }
}

export interface WebApi {
  publicProfile(slug: string): Promise<ProfileResult>;
  signedIn(): boolean;
  requestOtp(email: string): Promise<void>;
  verifyOtp(email: string, code: string): Promise<void>;
  signOut(): void;
  dashboard(): Promise<DashboardData>;
}

/** Where the session lives. The bundle uses sessionStorage, so closing the tab signs out. */
export interface TokenStore {
  get(): string | null;
  set(token: string | null): void;
}

export function memoryTokens(): TokenStore {
  let token: string | null = null;
  return { get: () => token, set: (t) => (token = t) };
}

export function sessionTokens(storage: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>): TokenStore {
  const key = 'sm.web.token';
  return {
    get: () => storage.getItem(key),
    set: (t) => (t === null ? storage.removeItem(key) : storage.setItem(key, t)),
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

type LiveConfig = Extract<WebConfig, { mode: 'live' }>;

export class WebApiClient implements WebApi {
  constructor(
    private readonly config: LiveConfig,
    private readonly fetchFn: typeof fetch,
    private readonly tokens: TokenStore,
  ) {}

  signedIn(): boolean {
    return this.tokens.get() !== null;
  }

  signOut(): void {
    this.tokens.set(null);
  }

  private async send(path: string, init: { method: string; body?: unknown; auth: boolean }): Promise<unknown> {
    const headers: Record<string, string> = { apikey: this.config.anonKey, Accept: 'application/json' };
    headers.Authorization = `Bearer ${init.auth ? (this.tokens.get() ?? this.config.anonKey) : this.config.anonKey}`;
    if (init.body !== undefined) headers['Content-Type'] = 'application/json';
    const response = await this.fetchFn(`${this.config.endpoint}${path}`, {
      method: init.method,
      headers,
      body: init.body === undefined ? undefined : JSON.stringify(init.body),
      credentials: 'omit',
      referrerPolicy: 'no-referrer',
    });
    if (response.status === 401) this.tokens.set(null);
    if (!response.ok) throw new ApiError(response.status);
    const text = await response.text();
    return text === '' ? null : (JSON.parse(text) as unknown);
  }

  async publicProfile(slug: string): Promise<ProfileResult> {
    // Anonymous on purpose: a link visitor sees the same as a stranger.
    return decodePublicProfile(await this.send('/rest/v1/rpc/get_public_profile', { method: 'POST', body: { p_slug: slug }, auth: false }));
  }

  async requestOtp(email: string): Promise<void> {
    await this.send('/auth/v1/otp', { method: 'POST', body: { email, create_user: false }, auth: false });
  }

  async verifyOtp(email: string, code: string): Promise<void> {
    const answer = await this.send('/auth/v1/verify', { method: 'POST', body: { type: 'email', email, token: code }, auth: false });
    const token = isRecord(answer) && typeof answer.access_token === 'string' ? answer.access_token : null;
    if (token === null) throw new ApiError(0);
    this.tokens.set(token);
  }

  async dashboard(): Promise<DashboardData> {
    if (!this.signedIn()) throw new ApiError(401);
    const get = (query: string) => this.send(`/rest/v1/${query}`, { method: 'GET', auth: true });
    const [days, status, inventory] = await Promise.all([
      get('daily_summaries?select=day,study_min,plan_min&order=day.desc&limit=60'),
      get('member_status?select=level&limit=1'),
      get('inventory_public?select=species&limit=1'),
    ]);
    return decodeDashboard(days, status, inventory);
  }
}

/** Fixed data for development: one profile whose private sections are absent, as the server would answer. */
export const MOCK_SLUG = '0123456789abcdef';

export function mockApi(today: string): WebApi {
  let signed = false;
  const day = (offset: number) => new Date(Date.parse(`${today}T00:00:00Z`) - offset * 86_400_000).toISOString().slice(0, 10);
  return {
    async publicProfile(slug) {
      if (slug !== MOCK_SLUG) return { kind: 'not_found' };
      return decodePublicProfile({
        code: 'ok',
        name: '민수',
        sections: {
          level: 7,
          grass: [3, 2, 1, 0].map((i) => ({ day: day(i), study_min: 30 * (4 - i) })),
          badges: ['streak_7'],
        },
      });
    },
    signedIn: () => signed,
    async requestOtp() {},
    async verifyOtp() {
      signed = true;
    },
    signOut() {
      signed = false;
    },
    async dashboard() {
      if (!signed) throw new ApiError(401);
      return { level: 7, species: 'fox', days: [0, 1, 2, 3, 4].map((i) => ({ day: day(i), studyMin: 60 + i * 10, planMin: 120 })) };
    },
  };
}
