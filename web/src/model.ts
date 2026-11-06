// What the web shows. Types are decoded from server answers and hold only the sections the
// server chose to return; there is no field for attempts, apps, titles or points.

export const SECTION_KEYS = ['room', 'grass', 'level', 'badges', 'guestbook', 'visitors', 'wiki', 'subject', 'gifts'] as const;
export type SectionKey = (typeof SECTION_KEYS)[number];

export interface RoomLook {
  species: string | null;
  hat: string | null;
  top: string | null;
  accessory: string | null;
  hand: string | null;
  furniture: string[];
}

export interface GrassDay {
  day: string;
  studyMin: number;
}

export interface PublicProfile {
  name: string;
  /** Sections the server returned, in display order. A hidden section is simply absent. */
  shown: SectionKey[];
  room: RoomLook | null;
  grass: GrassDay[] | null;
  level: number | null;
  badges: string[] | null;
  wiki: string[] | null;
  subject: string | null;
  visitors: { total: number; today: number } | null;
  gifts: { itemId: string; senderName: string; message: string }[] | null;
}

export type ProfileResult = { kind: 'ok'; profile: PublicProfile } | { kind: 'not_found' };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

const str = (v: unknown): string | null => (typeof v === 'string' ? v : null);
const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null);

function strings(v: unknown): string[] | null {
  return Array.isArray(v) && v.every((x) => typeof x === 'string') ? (v as string[]) : null;
}

/**
 * Decodes the answer of `get_public_profile`. Anything but `code: "ok"` is "not found"; a section
 * that is absent, malformed or unknown is dropped, so a server change can never make the page
 * show something it did not understand.
 */
export function decodePublicProfile(answer: unknown): ProfileResult {
  if (!isRecord(answer) || answer.code !== 'ok' || !isRecord(answer.sections)) return { kind: 'not_found' };
  const name = str(answer.name);
  if (name === null) return { kind: 'not_found' };
  const s = answer.sections;
  const profile: PublicProfile = { name, shown: [], room: null, grass: null, level: null, badges: null, wiki: null, subject: null, visitors: null, gifts: null };
  const mark = (key: SectionKey) => profile.shown.push(key);

  for (const key of SECTION_KEYS) {
    if (!(key in s)) continue;
    const value = s[key];
    switch (key) {
      case 'room':
        if (isRecord(value)) {
          profile.room = {
            species: str(value.species),
            hat: str(value.hat),
            top: str(value.top),
            accessory: str(value.accessory),
            hand: str(value.hand),
            furniture: strings(value.furniture) ?? [],
          };
          mark(key);
        }
        break;
      case 'grass':
        if (Array.isArray(value)) {
          profile.grass = value.flatMap((d) => {
            if (!isRecord(d)) return [];
            const day = str(d.day);
            const min = num(d.study_min);
            return day !== null && min !== null ? [{ day, studyMin: min }] : [];
          });
          mark(key);
        }
        break;
      case 'level':
        profile.level = num(value);
        mark(key);
        break;
      case 'badges':
        profile.badges = strings(value) ?? [];
        mark(key);
        break;
      case 'wiki':
        profile.wiki = strings(value) ?? [];
        mark(key);
        break;
      case 'subject':
        profile.subject = str(value);
        mark(key);
        break;
      case 'visitors':
        if (isRecord(value) && num(value.total) !== null && num(value.today) !== null) {
          profile.visitors = { total: num(value.total) ?? 0, today: num(value.today) ?? 0 };
          mark(key);
        }
        break;
      case 'gifts':
        if (Array.isArray(value)) {
          profile.gifts = value.flatMap((g) => {
            if (!isRecord(g)) return [];
            const itemId = str(g.item_id);
            const senderName = str(g.sender_name);
            return itemId !== null && senderName !== null ? [{ itemId, senderName, message: str(g.message) ?? '' }] : [];
          });
          mark(key);
        }
        break;
      case 'guestbook':
        // The guestbook is written and moderated in the app; the public page never lists it.
        break;
    }
  }
  return { kind: 'ok', profile };
}

// ---- dashboard ----

export interface DayRow {
  day: string;
  studyMin: number;
  planMin: number;
}

export interface DashboardData {
  level: number | null;
  species: string | null;
  days: DayRow[];
}

export interface DashboardSummary {
  weekStudyMin: number;
  /** Study minutes over plan minutes for the last 7 days, 0-100; null when nothing was planned. */
  weekAchievementPct: number | null;
  rows: DayRow[];
}

function dayAdd(day: string, delta: number): string {
  return new Date(Date.parse(`${day}T00:00:00Z`) + delta * 86_400_000).toISOString().slice(0, 10);
}

/** The last 7 calendar days ending at `today` (YYYY-MM-DD). Days with no row count as zero. */
export function summarize(days: readonly DayRow[], today: string): DashboardSummary {
  const from = dayAdd(today, -6);
  const week = days.filter((d) => d.day >= from && d.day <= today);
  const study = week.reduce((a, d) => a + d.studyMin, 0);
  const plan = week.reduce((a, d) => a + d.planMin, 0);
  return {
    weekStudyMin: study,
    weekAchievementPct: plan > 0 ? Math.min(100, Math.round((study / plan) * 100)) : null,
    rows: [...days].sort((a, b) => (a.day < b.day ? 1 : -1)),
  };
}

export function decodeDays(rows: unknown): DayRow[] {
  if (!Array.isArray(rows)) return [];
  return rows.flatMap((r) => {
    if (!isRecord(r)) return [];
    const day = str(r.day);
    const study = num(r.study_min);
    const plan = num(r.plan_min);
    return day !== null && study !== null && plan !== null ? [{ day, studyMin: study, planMin: plan }] : [];
  });
}

export function decodeDashboard(daysRows: unknown, statusRows: unknown, inventoryRows: unknown): DashboardData {
  const first = (rows: unknown) => (Array.isArray(rows) && isRecord(rows[0]) ? rows[0] : null);
  return {
    level: num(first(statusRows)?.level),
    species: str(first(inventoryRows)?.species),
    days: decodeDays(daysRows),
  };
}

// ---- routes ----

export type Route = { kind: 'home' } | { kind: 'profile'; slug: string } | { kind: 'dashboard' };

export function parseRoute(hash: string): Route {
  const profile = /^#\/u\/([a-f0-9]{16})$/.exec(hash);
  if (profile?.[1] !== undefined) return { kind: 'profile', slug: profile[1] };
  if (hash === '#/dashboard') return { kind: 'dashboard' };
  return { kind: 'home' };
}
