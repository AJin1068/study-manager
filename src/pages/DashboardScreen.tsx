import { useEffect, useState, type ReactNode } from 'react';
import {
  commands,
  type GrowthStatus,
  type ModeSnapshot,
  type TimerPhase,
  type TodayDashboard,
  type WeekDashboard,
  type Weakness,
  type WeeklyInsight,
} from '../api';
import { useAsync } from '../hooks/useAsync';
import { asyncStatus } from '../hooks/useAsyncStatus';
import { useStudyLock } from '../hooks/useStudyLock';
import { useTimerState } from '../hooks/useTimerState';
import {
  WEEKDAY_LABELS,
  appOffText,
  barPercent,
  durationText,
  firstAwayText,
  heatLevel,
  heatmapIsEmpty,
  heatmapMax,
  nextPlanText,
  pctText,
  phonePickupText,
  phoneUnreachableText,
  releaseText,
  scoreText,
  streakText,
} from './dashboardView';
import { insightNote, reviewText } from './growthView';
import { CharacterAvatar } from '../components/CharacterAvatar';
import { FocusSignalsPanel } from '../components/FocusSignalsPanel';
import { PhoneLinkNotice } from '../components/PhoneLinkNotice';
import { TimerPanel } from '../components/TimerPanel';
import { TopicPanel } from '../components/TopicPanel';
import { GrowthTab } from './GrowthTab';
import { ModeCard } from './MainScreen';

const DATE_FMT = new Intl.DateTimeFormat('ko-KR', { month: 'long', day: 'numeric', weekday: 'long' });

/** Shows the loading state or the error (even over stale data), then the value. */
function Loaded<T>(props: { data: T | null; error: string | null; children: (value: T) => ReactNode }) {
  const status = asyncStatus(props.data, props.error);
  return (
    <>
      {status.kind === 'loading' && <p className="hint">불러오는 중</p>}
      {status.kind === 'error' && <p className="error">읽지 못했습니다: {status.message}</p>}
      {status.kind === 'ready' && props.children(status.value)}
      {status.kind === 'error' && status.stale !== null && props.children(status.stale)}
    </>
  );
}

/** Minutes of the running segment under today's mode; drives only the "지금" progress bar. */
function segmentMinutes(phase: TimerPhase | undefined, modes: ModeSnapshot | null): number | null {
  if (phase === undefined || modes === null) return null;
  const config = modes.configs.find((c) => c.level === modes.today.level);
  if (!config) return null;
  if (phase === 'study') return config.studyMin;
  if (phase === 'break') return config.breakMin;
  if (phase === 'longBreak') return config.longBreakMin;
  return null;
}

function TodayCard({ today }: { today: TodayDashboard | null }) {
  return (
    <section className="card" aria-labelledby="today-title">
      <h2 id="today-title">오늘 계획 대비 실제</h2>
      {today === null ? (
        <p className="hint">불러오는 중</p>
      ) : (
        <>
          <div className="kv">
            <span>계획</span>
            <b>{durationText(today.plannedMs)}</b>
            <span>실제 공부</span>
            <b>{durationText(today.actualMs)}</b>
            <span>달성률</span>
            <b>{pctText(today.achievementPct)}</b>
          </div>
          <div className="prog spaced" role="img" aria-label={`오늘 달성 ${pctText(today.achievementPct)}`}>
            <span style={{ width: `${barPercent(today.actualMs, today.plannedMs)}%` }} />
          </div>
          <p className="hint">{nextPlanText(today)}</p>
        </>
      )}
    </section>
  );
}

function LevelCard({ today }: { today: TodayDashboard | null }) {
  const character = useAsync(commands.getCharacter);
  const view = character.data;
  const points = today?.points ?? null;
  const toNext = points !== null && points.nextLevelAt !== null ? points.nextLevelAt - points.earned : null;
  const pct =
    points !== null && points.nextLevelAt !== null && points.nextLevelAt > 0
      ? Math.min(100, Math.round((points.earned / points.nextLevelAt) * 100))
      : 100;
  return (
    <section className="card" aria-labelledby="level-title">
      <h2 id="level-title">레벨</h2>
      <div className="level-row">
        {view && <CharacterAvatar svg={view.svg} label={`${view.name} (${view.stageLabel})`} size={64} />}
        <div>
          <div className="level-name">{points !== null ? `Lv.${points.level} ${points.stageLabel}` : '불러오는 중'}</div>
          <div className="hint">
            {view ? `${view.name} · ` : ''}
            {toNext !== null ? `다음 레벨까지 ${toNext}P` : '최고 레벨이에요'}
          </div>
        </div>
      </div>
      <div className="prog ember spaced" role="img" aria-label={`레벨 진행 ${pct}%`}>
        <span style={{ width: `${pct}%` }} />
      </div>
      {points !== null && <p className="hint">보유 {points.balance.toLocaleString()}P · 30분 달성할 때마다 포인트가 쌓여요</p>}
    </section>
  );
}

function InsightBanner() {
  const [insight, setInsight] = useState<WeeklyInsight | null>(null);
  useEffect(() => {
    commands.getWeeklyInsight(false).then(setInsight).catch(() => setInsight(null));
  }, []);
  if (insight === null || insight.text === '') return null;
  const note = insightNote(insight);
  return (
    <div className="insight" role="note">
      <svg width="20" height="20" viewBox="0 0 20 20" aria-hidden="true">
        <circle cx="10" cy="10" r="9" fill="var(--accent)" />
        <rect x="9" y="5" width="2" height="7" rx="1" fill="var(--win)" />
        <rect x="9" y="13.5" width="2" height="2" rx="1" fill="var(--win)" />
      </svg>
      <div>
        <strong>이번 주 인사이트</strong>
        <br />
        {insight.text}
        {note !== null && <div className="hint">{note}</div>}
      </div>
    </div>
  );
}

/** Planned (outline) vs actual (filled) hours per weekday, as in the mockup. */
function WeekChart({ week }: { week: WeekDashboard }) {
  const maxMs = Math.max(1, ...week.days.map((d) => Math.max(d.plannedMs, d.actualMs)));
  const maxH = Math.max(2, Math.ceil(maxMs / 3_600_000));
  const top = 20;
  const base = 150;
  const h = (ms: number) => ((base - top) * ms) / (maxH * 3_600_000);
  const byWeekday = new Map(week.days.map((d) => [d.weekday, d]));
  return (
    <svg viewBox="0 0 420 180" width="100%" role="img" aria-label="요일별 계획과 실제 공부 시간">
      <g stroke="var(--line)">
        <line x1="30" x2="410" y1={top} y2={top} />
        <line x1="30" x2="410" y1={(top + base) / 2} y2={(top + base) / 2} />
        <line x1="30" x2="410" y1={base} y2={base} stroke="var(--muted)" />
      </g>
      <text x="22" y={top + 4} textAnchor="end">{maxH}</text>
      <text x="22" y={(top + base) / 2 + 4} textAnchor="end">{maxH / 2}</text>
      <text x="22" y={base + 4} textAnchor="end">0</text>
      {WEEKDAY_LABELS.map((label, i) => {
        const d = byWeekday.get(i);
        const x = 44 + i * 54;
        const p = d ? h(d.plannedMs) : 0;
        const a = d ? h(d.actualMs) : 0;
        return (
          <g key={label}>
            {p > 0 && <rect x={x} y={base - p} width="18" height={p} rx="3" fill="var(--side)" stroke="var(--line)" />}
            {a > 0 && <rect x={x + 20} y={base - a} width="18" height={a} rx="3" fill="var(--accent)" />}
            <text x={x + 19} y="168" textAnchor="middle">{label}</text>
            {d && (
              <title>
                {label} 계획 {durationText(d.plannedMs)} · 실제 {durationText(d.actualMs)} · 집중 {scoreText(d.focusScore)}
              </title>
            )}
          </g>
        );
      })}
    </svg>
  );
}

function WeekCard({ week }: { week: WeekDashboard | null }) {
  return (
    <section className="card" aria-labelledby="week-title">
      <h2 id="week-title">이번 주 계획 대비 실제 (시간)</h2>
      {week === null ? (
        <p className="hint">불러오는 중</p>
      ) : (
        <>
          <WeekChart week={week} />
          <div className="legend">
            <span>
              <i className="key-plan" /> 계획
            </span>
            <span>
              <i className="key-actual" /> 실제
            </span>
            <span>
              달성률 <b className="mono">{pctText(week.achievementPct)}</b>
            </span>
            <span>
              달성한 날 <b className="mono">{week.achievedDays}/{week.plannedDays}</b>
            </span>
          </div>
        </>
      )}
    </section>
  );
}

/** The 8-hour window with the most recorded distractions (evening when there are none). */
export function heatWindow(heatmap: number[][]): number[] {
  const width = 8;
  let best = 16;
  let bestSum = 0;
  for (let start = 0; start <= 24 - width; start += 1) {
    let sum = 0;
    for (const row of heatmap) for (let h = start; h < start + width; h += 1) sum += row[h] ?? 0;
    if (sum > bestSum) {
      best = start;
      bestSum = sum;
    }
  }
  return Array.from({ length: width }, (_, i) => best + i);
}

function HeatCard({ weak }: { weak: Weakness | null }) {
  if (weak === null) {
    return (
      <section className="card" aria-labelledby="heat-title">
        <h2 id="heat-title">시간대별 딴짓 시도 (최근 4주)</h2>
        <p className="hint">불러오는 중</p>
      </section>
    );
  }
  const hours = heatWindow(weak.heatmap);
  const max = heatmapMax(weak.heatmap);
  return (
    <section className="card" aria-labelledby="heat-title">
      <h2 id="heat-title">시간대별 딴짓 시도 (최근 4주)</h2>
      {heatmapIsEmpty(weak.heatmap) && <p className="hint">아직 기록된 딴짓 시도가 없어요.</p>}
      <div className="heat" role="table" aria-label="요일과 시간대별 딴짓 시도 횟수">
        <div className="heat-line" role="row">
          <span />
          {hours.map((h) => (
            <span key={h} role="columnheader">
              {h}
            </span>
          ))}
        </div>
        {weak.heatmap.map((row, weekday) => (
          <div className="heat-line" role="row" key={weekday}>
            <span role="rowheader">{WEEKDAY_LABELS[weekday] ?? '?'}</span>
            {hours.map((h) => (
              <i
                key={h}
                role="cell"
                data-level={heatLevel(row[h] ?? 0, max)}
                title={`${WEEKDAY_LABELS[weekday] ?? '?'} ${h}시 ${row[h] ?? 0}회`}
              />
            ))}
          </div>
        ))}
      </div>
    </section>
  );
}

function focusScoreAvg(week: WeekDashboard | null): number | null {
  if (week === null) return null;
  const scores = week.days.map((d) => d.focusScore).filter((s): s is number => s !== null);
  if (scores.length === 0) return null;
  return Math.round(scores.reduce((a, b) => a + b, 0) / scores.length);
}

function BottomRow(props: { weak: Weakness | null; week: WeekDashboard | null; growth: GrowthStatus | null }) {
  const { weak, week, growth } = props;
  return (
    <div className="grid g-bot">
      <section className="card" aria-labelledby="apps-title">
        <h2 id="apps-title">딴짓 시도 앱 순위</h2>
        {weak !== null && weak.deviationApps.length === 0 && <p className="hint">기록된 딴짓 시도가 없어요.</p>}
        {weak !== null && weak.deviationApps.length > 0 && (
          <ul className="rank">
            {weak.deviationApps.slice(0, 5).map((a) => (
              <li key={a.appId}>
                <span>{a.appId}</span>
                <b className="mono">{a.count}회</b>
              </li>
            ))}
          </ul>
        )}
      </section>
      <section className="card" aria-labelledby="pattern-title">
        <h2 id="pattern-title">집중 패턴</h2>
        <div className="kv">
          <span>이번 주 집중 점수</span>
          <b>{scoreText(focusScoreAvg(week))}</b>
          <span>이번 주 공부</span>
          <b>{week === null ? '-' : durationText(week.actualMs)}</b>
          <span>공부한 과목</span>
          <b>{week === null ? '-' : `${week.bySubject.length}개`}</b>
        </div>
        {weak !== null && <p className="hint">{firstAwayText(weak.avgFirstAwayMs, weak.setsWithAway, weak.setsTotal)}</p>}
      </section>
      <section className="card" aria-labelledby="review-title">
        <h2 id="review-title">다음 복습 추천 (위키)</h2>
        {growth === null ? (
          <p className="hint">불러오는 중</p>
        ) : (
          <>
            <p className="review-line">{reviewText(growth.nextReview)}</p>
            <div className="kv">
              <span>위키 페이지</span>
              <b>{growth.wikiPages}개</b>
              <span>퀴즈 정답률</span>
              <b>{growth.quizAccuracyPct === null ? '-' : `${growth.quizAccuracyPct}%`}</b>
            </div>
          </>
        )}
      </section>
    </div>
  );
}

type Detail = 'week' | 'weakness' | 'growth';
const DETAIL_LABEL: Record<Detail, string> = { week: '과목별 시간', weakness: '약점 자세히', growth: '성장' };

function WeekDetail({ week }: { week: WeekDashboard }) {
  return (
    <section className="card" aria-labelledby="subject-title">
      <h2 id="subject-title">
        과목별 시간 ({week.weekStart} ~ {week.weekEnd})
      </h2>
      {week.bySubject.length === 0 && <p className="hint">이번 주 인정된 공부 시간이 없습니다.</p>}
      <ul className="rank">
        {week.bySubject.map((s) => (
          <li key={s.subject}>
            <span>{s.subject}</span>
            <b className="mono">{durationText(s.actualMs)}</b>
          </li>
        ))}
      </ul>
    </section>
  );
}

function WeaknessDetail({ weak }: { weak: Weakness }) {
  return (
    <div className="grid g-two">
      <section className="card" aria-labelledby="postponed-title">
        <h2 id="postponed-title">자주 밀리는 계획</h2>
        {weak.postponedPlans.length === 0 && <p className="hint">2번 이상 미룬 계획이 없습니다.</p>}
        <ul className="rank">
          {weak.postponedPlans.map((p) => (
            <li key={p.planId}>
              <span>
                {p.name} <small className="hint">{p.subject}</small>
              </span>
              <span className="pill p-break">{p.count}회 미룸</span>
            </li>
          ))}
        </ul>
      </section>
      <section className="card" aria-labelledby="off-title">
        <h2 id="off-title">앱이 꺼져 있던 시간</h2>
        <p>{appOffText(weak.appOff)}</p>
        <p className="hint">절전과 앱 종료는 집중 점수에서 빼고 따로 보여줍니다.</p>
      </section>
      <section className="card" aria-labelledby="phone-title">
        <h2 id="phone-title">폰</h2>
        <p>{phoneUnreachableText(weak.phone)}</p>
        <p>{phonePickupText(weak.phone)}</p>
        <p className="hint">통계로만 보여주며 집중 점수와 포인트에는 영향이 없습니다. 폰은 횟수와 구간 번호만 보냅니다.</p>
      </section>
      <section className="card" aria-labelledby="release-title">
        <h2 id="release-title">해제 사용 기록</h2>
        <p>{releaseText(weak.releases)}</p>
      </section>
    </div>
  );
}

/**
 * Home screen, laid out like docs/design/mockup.html: now / today / level on top, the weekly
 * insight, the week chart and heatmap, then apps, focus pattern and review. Today's mode, focus
 * signals and topic follow, and the detailed views open on demand.
 */
export function DashboardScreen() {
  const lock = useStudyLock();
  const timer = useTimerState();
  const modes = useAsync(commands.getModeConfig);
  const today = useAsync(commands.getDashboardToday);
  const week = useAsync(commands.getDashboardWeek);
  const weak = useAsync(commands.getWeakness);
  const growth = useAsync(commands.getGrowthStatus);
  const [detail, setDetail] = useState<Detail | null>(null);

  const timerLockHeld = timer.state === null ? null : timer.state.lockHeld;
  const { refresh: refreshLock } = lock;
  const { reload: reloadToday } = today;
  useEffect(() => {
    // A run started or ended: the lock and today's numbers changed.
    if (timerLockHeld === null) return;
    refreshLock();
    reloadToday();
  }, [timerLockHeld, refreshLock, reloadToday]);

  const t = today.data;
  const heading = DATE_FMT.format(t !== null ? new Date(`${t.date}T00:00:00`) : new Date());
  const statsError = week.error ?? weak.error ?? growth.error;

  return (
    <main className="page dash">
      <header>
        <h1>오늘, {heading}</h1>
        <p className="sub">{t !== null ? streakText(t) : '오늘 기록을 불러오는 중'}</p>
        {today.error !== null && <p className="error">오늘 기록을 읽지 못했습니다: {today.error}</p>}
      </header>

      <div className="grid g-top">
        <section className="card now-card" aria-label="지금">
          <TimerPanel timer={timer} segmentMin={segmentMinutes(timer.state?.phase, modes.data)} />
        </section>
        <TodayCard today={t} />
        <LevelCard today={t} />
      </div>

      <PhoneLinkNotice />
      <InsightBanner />

      <div className="grid g-mid">
        <WeekCard week={week.data} />
        <HeatCard weak={weak.data} />
      </div>
      {statsError !== null && <p className="error">일부 통계를 읽지 못했습니다: {statsError}</p>}

      <BottomRow weak={weak.data} week={week.data} growth={growth.data} />

      <div className="grid g-mid">
        <ModeCard lock={lock} modes={modes} />
        <FocusSignalsPanel timer={timer} />
      </div>
      <TopicPanel />

      <div className="row" role="group" aria-label="자세히 보기">
        {(Object.keys(DETAIL_LABEL) as Detail[]).map((key) => (
          <button key={key} type="button" aria-pressed={detail === key} onClick={() => setDetail(detail === key ? null : key)}>
            {DETAIL_LABEL[key]}
          </button>
        ))}
      </div>
      {detail === 'week' && (
        <Loaded<WeekDashboard> data={week.data} error={week.error}>
          {(w) => <WeekDetail week={w} />}
        </Loaded>
      )}
      {detail === 'weakness' && (
        <Loaded<Weakness> data={weak.data} error={weak.error}>
          {(w) => <WeaknessDetail weak={w} />}
        </Loaded>
      )}
      {detail === 'growth' && <GrowthTab />}
    </main>
  );
}
