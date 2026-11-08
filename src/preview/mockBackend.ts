// Browser-only preview backend: answers Tauri commands with example data so the UI can be laid out
// and screenshotted with `pnpm dev` at /preview.html. Never loaded by the app build (index.html).
import { mockIPC } from '@tauri-apps/api/mocks';
import dashboard from '../api/fixtures/dashboard-contract.json';
import character from '../api/fixtures/character-contract.json';
import points from '../api/fixtures/points-contract.json';
import modeConfig from '../api/fixtures/mode-config.json';
import generalSettings from '../api/fixtures/general-settings.json';
import plan from '../api/fixtures/plan-contract.json';
import growth from '../api/fixtures/growth-contract.json';
import enforce from '../api/fixtures/enforce-contract.json';

const HOUR = 3_600_000;
const MIN = 60_000;

function todayIso(offsetDays = 0): string {
  const d = new Date();
  d.setDate(d.getDate() + offsetDays);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

function at(dateIso: string, hhmm: string): number {
  return new Date(`${dateIso}T${hhmm}:00`).getTime();
}

const AVATAR =
  '<svg viewBox="0 0 32 32" xmlns="http://www.w3.org/2000/svg"><rect x="4" y="8" width="24" height="20" rx="8" fill="#5fb39a"/><rect x="10" y="15" width="3" height="4" rx="1.5" fill="#161c1b"/><rect x="19" y="15" width="3" height="4" rx="1.5" fill="#161c1b"/></svg>';

function occurrences() {
  const monday = (() => {
    const d = new Date();
    const day = (d.getDay() + 6) % 7;
    d.setDate(d.getDate() - day);
    return d;
  })();
  const iso = (n: number) => {
    const d = new Date(monday);
    d.setDate(d.getDate() + n);
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
  };
  const rows: [number, string, string, string, number, boolean][] = [
    [0, '자료구조 1강 (예시)', '자료구조', '20:00', 2, false],
    [1, '운영체제 2강 (예시)', '운영체제', '20:00', 2, false],
    [2, '운영체제 3강 (예시)', '운영체제', '20:00', 2, false],
    [3, '네트워크 1강 (예시)', '네트워크', '20:00', 2, false],
    [4, '자료구조 2강 (예시)', '자료구조', '20:00', 1, true],
    [5, '알고리즘 문제 (예시)', '알고리즘', '19:00', 1, false],
    [6, '주간 복습 (예시)', '복습', '19:00', 1, false],
  ];
  return rows.map(([day, name, subject, time, sets, moved], i) => ({
    ...plan.occurrence,
    planId: 10 + i,
    seriesId: 10 + i,
    name,
    subject,
    date: iso(day),
    sourceDate: moved ? iso(2) : iso(day),
    startTime: time,
    sets,
    moved,
    durationMin: sets * 60,
    plannedStartMs: at(iso(day), time),
    plannedEndMs: at(iso(day), time) + sets * HOUR,
  }));
}

function handlers(): Record<string, (args: Record<string, unknown>) => unknown> {
  const now = Date.now();
  const today = todayIso();
  return {
    get_lock_state: () => ({ lockHeld: true, changedAtMs: now - 28 * MIN }),
    get_mode_config: () => ({
      today: { date: today, level: 'medium', stored: true },
      configs: [...modeConfig, { level: 'hard', studyMin: 90, breakMin: 10, longBreakMin: null, setsBeforeLongBreak: null }],
    }),
    get_timer_state: () => ({
      phase: 'study',
      runId: 4,
      segmentId: 19,
      setIndex: 2,
      setsTotal: 3,
      plannedEndMs: now + 32 * MIN + 15_000,
      lockHeld: true,
      nowMs: now,
      next: null,
    }),
    get_engine_health: () => ({ degraded: false, consecutiveFailures: 0, restarts: 0, clockJumpMs: 0 }),
    get_character: () => ({ ...character.view, name: '아진 (예시)', svg: AVATAR, level: 7, stageLabel: '집중 가재' }),
    get_points_status: () => ({ ...points.status, balance: 1240, earned: 1380, level: 7, nextLevelAt: 1640 }),
    list_point_ledger: () => [
      { id: 4, atMs: now - 10 * MIN, delta: 20, reasonCode: 'focus_window', refKey: 'a' },
      { id: 3, atMs: now - 60 * MIN, delta: 10, reasonCode: 'start_bonus', refKey: 'b' },
      { id: 2, atMs: now - 2 * HOUR, delta: 15, reasonCode: 'quiz_bonus', refKey: 'c' },
      { id: 1, atMs: now - 26 * HOUR, delta: -10, reasonCode: 'emergency_pass', refKey: 'd' },
    ],
    get_dashboard_today: () => ({
      ...dashboard.today,
      date: today,
      plannedMs: 4 * HOUR,
      actualMs: 108 * MIN,
      achievementPct: 45,
      streakDays: 6,
      nextPlan: { ...dashboard.today.nextPlan, name: '운영체제 3강 (예시)', subject: '운영체제', plannedStartMs: now + 2 * HOUR, startsInMs: 2 * HOUR },
      points: { ...dashboard.today.points, balance: 1240, earned: 1380, level: 7, nextLevelAt: 1640, stageLabel: '집중 가재' },
    }),
    get_dashboard_week: () => {
      const planned = [4, 3, 4, 3, 3, 2, 2];
      const actual = [3.2, 3, 1.8, 0, 0, 0, 0];
      return {
        ...dashboard.week,
        days: planned.map((p, i) => ({
          date: todayIso(i - 2),
          weekday: i,
          plannedMs: p * HOUR,
          actualMs: actual[i] * HOUR,
          achievementPct: Math.round((actual[i] / p) * 100),
          achieved: actual[i] / p >= 0.8,
          focusScore: actual[i] > 0 ? 70 + i * 3 : null,
        })),
        plannedMs: 21 * HOUR,
        actualMs: 8 * HOUR,
        achievementPct: 78,
        bySubject: [
          { subject: '운영체제', actualMs: 4 * HOUR },
          { subject: '자료구조', actualMs: 3 * HOUR },
        ],
      };
    },
    get_weakness: () => ({
      ...dashboard.weakness,
      deviationApps: [
        { appId: '유튜브 (크롬)', count: 14 },
        { appId: '카카오톡', count: 9 },
        { appId: '디스코드', count: 5 },
        { appId: '스팀', count: 2 },
      ],
      avgFirstAwayMs: 14 * MIN,
      heatmap: Array.from({ length: 7 }, (_, d) => Array.from({ length: 24 }, (_, h) => (h >= 18 && h <= 23 ? (d * 3 + h) % 5 : 0))),
    }),
    get_growth_status: () => ({ ...growth.status, level: 7, stageLabel: '집중 가재', nextReview: { subject: '운영체제', pageName: '페이지 교체 알고리즘 (예시)', reason: 'low_accuracy' } }),
    get_weekly_insight: () => ({
      weekStart: todayIso(-2),
      text: '화요일 밤 9시대 집중 점수가 가장 높았어요. 토요일 오후 계획은 3번 중 2번 밀렸어요. 토요일은 1시간으로 줄여볼까요?',
      source: 'rule',
      fallbackReason: 'llm_disabled',
    }),
    get_insight_subject: () => false,
    get_settings: () => generalSettings,
    get_autostart_status: () => ({ enabled: true }),
    get_power_status: () => ({ active: true, reason: null }),
    get_setting_history: () => [
      { id: 2, atMs: now - HOUR, key: 'relaunchOnKill', before: 'false', after: 'true', reason: 'user', outcome: 'applied' },
    ],
    get_downtime_records: () => [],
    list_occurrences: () => occurrences(),
    get_plan_history: () => [
      { ...plan.historyEntry, id: 3, atMs: now - 2 * HOUR },
      { ...plan.historyEntry, id: 2, action: 'create', reasonCode: null, beforeJson: null, atMs: now - 30 * HOUR },
    ],
    get_plan_stats: () => ({ ...plan.stats, bySubject: [{ subject: '자료구조', count: 2 }] }),
    get_calendar_io_enabled: () => false,
    get_enforcement_status: () => enforce.status,
    get_pass_status: () => ({ dailyLimit: 1, usedToday: 0, remainingToday: 1, activeUntilMs: null, durationMin: 10, penaltyPoints: 10 }),
    get_release_quiz_status: () => ({ mode: 'quiz', required: 3, drawable: 12, attemptsUsed: 0, maxAttempts: 4, cooldownRemainingMs: 0, cooldownMin: 3 }),
    get_focus_setup: () => ({ studyAppCount: 3, needsStudyApps: false, titleAccess: 'unsupported', sensorsAvailable: true }),
    get_self_report_status: () => ({ shortcut: 'Ctrl+Alt+Shift+D', shortcutRegistered: true, shortcutError: null }),
    get_pending_checkin: () => null,
    get_phone_link: () => ({ paired: false, wantLocked: true, published: null, publishFailing: false, lastError: null, openGaps: [], lastBeatMs: null, serviceEnabled: null }),
    get_browser_link_status: () => ({ transportActive: true, transportError: null, connected: true, connections: 1, lastConnectedAtMs: now - HOUR, lastDisconnectedAtMs: null, lastDisconnectReason: null }),
    get_chat_draft: () => null,
    get_session_end_quiz: () => null,
    get_checkin_quiz: () => null,
    get_shop: () => ({
      balance: 1240,
      items: [
        { id: 'hat_beanie', kind: 'clothing', slot: 'hat', label: '비니 (예시)', price: 40, source: 'shop', giftable: true, achievement: null, owned: true, equipped: true, affordable: true },
        { id: 'top_vest', kind: 'clothing', slot: 'top', label: '조끼 (예시)', price: 800, source: 'shop', giftable: true, achievement: null, owned: false, equipped: false, affordable: true },
        { id: 'hand_pen', kind: 'clothing', slot: 'hand', label: '만년필 (예시)', price: 1500, source: 'shop', giftable: true, achievement: null, owned: false, equipped: false, affordable: false },
        { id: 'acc_knit_scarf', kind: 'clothing', slot: 'accessory', label: '털실 목도리 (예시)', price: 0, source: 'achievement', giftable: false, achievement: '7일 연속 달성', owned: false, equipped: false, affordable: false },
      ],
    }),    get_today_topic: () => ({ planId: null, planName: null, date: today, source: null, assignedBy: null, reason: null, page: null, notice: null }),
    get_preset_status: () => ({ onboarded: true, version: 1, categories: [] }),  };
}

/** Installs the preview backend. Unknown commands fail with a visible reason instead of hanging. */
export function installPreviewBackend() {
  const table = handlers();
  mockIPC(
    (cmd, payload) => {
      const handler = table[cmd];
      if (handler) return handler((payload ?? {}) as Record<string, unknown>);
      if (cmd.startsWith('list_') || cmd.endsWith('_list')) return [];
      return Promise.reject(new Error(`미리보기 데이터 없음 (${cmd})`));
    },
    { shouldMockEvents: true },
  );
}
