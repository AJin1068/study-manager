import { useState, type ReactNode } from 'react';
import { commands, type TodayDashboard, type WeekDashboard, type Weakness } from '../api';
import { useAsync } from '../hooks/useAsync';
import { asyncStatus } from '../hooks/useAsyncStatus';
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
  releaseText,
  scoreText,
  streakText,
} from './dashboardView';
import { CharacterBadge } from '../components/CharacterBadge';
import { GrowthTab } from './GrowthTab';

type Tab = 'today' | 'week' | 'weakness' | 'growth';

const TAB_LABEL: Record<Tab, string> = { today: '오늘', week: '이번 주', weakness: '약점', growth: '성장' };

/** Shows the loading state or the error (even over stale data), then the value. */
function Loaded<T>(props: { data: T | null; error: string | null; children: (value: T) => ReactNode }) {
  const status = asyncStatus(props.data, props.error);
  return (
    <>
      {status.kind === 'loading' && <p>불러오는 중</p>}
      {status.kind === 'error' && <p className="error">읽지 못했습니다: {status.message}</p>}
      {status.kind === 'ready' && props.children(status.value)}
      {status.kind === 'error' && status.stale !== null && props.children(status.stale)}
    </>
  );
}

function TodayView() {
  const today = useAsync(commands.getDashboardToday);
  return (
    <Loaded<TodayDashboard> data={today.data} error={today.error}>
      {(t) => (
        <section className="card" aria-labelledby="today-title">
          <h2 id="today-title">오늘 ({t.date})</h2>
          <p>
            다음 계획: <strong>{nextPlanText(t)}</strong>
          </p>
          <p>
            계획 대비 실제: {durationText(t.actualMs)} / {durationText(t.plannedMs)} ({pctText(t.achievementPct)})
          </p>
          <div className="bar" role="img" aria-label={`오늘 달성 ${pctText(t.achievementPct)}`}>
            <div className="bar-fill" style={{ width: `${barPercent(t.actualMs, t.plannedMs)}%` }} />
          </div>
          <p>{streakText(t)}</p>
          <p>
            Lv.{t.points.level} {t.points.stageLabel} · 획득 {t.points.earned}P · 보유 {t.points.balance}P
            {t.points.nextLevelAt !== null && ` · 다음 레벨까지 ${t.points.nextLevelAt - t.points.earned}P`}
          </p>
        </section>
      )}
    </Loaded>
  );
}

function WeekView() {
  const week = useAsync(commands.getDashboardWeek);
  return (
    <Loaded<WeekDashboard> data={week.data} error={week.error}>
      {(w) => (
        <>
          <section className="card" aria-labelledby="week-title">
            <h2 id="week-title">
              이번 주 ({w.weekStart} ~ {w.weekEnd})
            </h2>
            <p>
              달성률 {pctText(w.achievementPct)} · {durationText(w.actualMs)} / {durationText(w.plannedMs)} · 달성한 날{' '}
              {w.achievedDays}/{w.plannedDays}
            </p>
            <ul className="week-days">
              {w.days.map((d) => (
                <li key={d.date}>
                  <span>{WEEKDAY_LABELS[d.weekday] ?? '?'}</span>
                  <div className="bar" role="img" aria-label={`${d.date} 계획 ${durationText(d.plannedMs)} 실제 ${durationText(d.actualMs)}`}>
                    <div className="bar-fill" style={{ width: `${barPercent(d.actualMs, d.plannedMs)}%` }} />
                  </div>
                  <small>
                    {durationText(d.actualMs)} / {durationText(d.plannedMs)} · 집중 {scoreText(d.focusScore)}
                  </small>
                </li>
              ))}
            </ul>
          </section>
          <section className="card" aria-labelledby="subject-title">
            <h2 id="subject-title">과목별 시간</h2>
            {w.bySubject.length === 0 && <p className="hint">이번 주 인정된 공부 시간이 없습니다.</p>}
            <ul className="list">
              {w.bySubject.map((s) => (
                <li key={s.subject}>
                  {s.subject}: {durationText(s.actualMs)}
                </li>
              ))}
            </ul>
          </section>
        </>
      )}
    </Loaded>
  );
}

function WeaknessView() {
  const weak = useAsync(commands.getWeakness);
  return (
    <Loaded<Weakness> data={weak.data} error={weak.error}>
      {(w) => (
        <>
          <section className="card" aria-labelledby="apps-title">
            <h2 id="apps-title">최근 4주 딴짓 앱 순위</h2>
            {w.deviationApps.length === 0 && <p className="hint">기록된 딴짓 시도가 없습니다.</p>}
            <ol className="list">
              {w.deviationApps.map((a) => (
                <li key={a.appId}>
                  {a.appId}: {a.count}회
                </li>
              ))}
            </ol>
            <p>{firstAwayText(w.avgFirstAwayMs, w.setsWithAway, w.setsTotal)}</p>
          </section>
          <section className="card" aria-labelledby="heat-title">
            <h2 id="heat-title">요일·시간대 히트맵</h2>
            {heatmapIsEmpty(w.heatmap) ? (
              <p className="hint">기록된 이탈이 없습니다.</p>
            ) : (
              <div className="heatmap" role="table" aria-label="요일과 시간대별 이탈 횟수">
                {w.heatmap.map((row, weekday) => (
                  <div className="heat-row" role="row" key={weekday}>
                    <span className="heat-label" role="rowheader">
                      {WEEKDAY_LABELS[weekday] ?? '?'}
                    </span>
                    {row.map((count, hour) => (
                      <span
                        key={hour}
                        role="cell"
                        className="heat-cell"
                        data-level={heatLevel(count, heatmapMax(w.heatmap))}
                        aria-label={`${WEEKDAY_LABELS[weekday] ?? '?'}요일 ${hour}시 ${count}회`}
                        title={`${hour}시 ${count}회`}
                      />
                    ))}
                  </div>
                ))}
              </div>
            )}
          </section>
          <section className="card" aria-labelledby="postponed-title">
            <h2 id="postponed-title">자주 밀리는 계획</h2>
            {w.postponedPlans.length === 0 && <p className="hint">2번 이상 미룬 계획이 없습니다.</p>}
            <ul className="list">
              {w.postponedPlans.map((p) => (
                <li key={p.planId}>
                  {p.name} ({p.subject}): {p.count}회 미룸
                </li>
              ))}
            </ul>
          </section>
          <section className="card" aria-labelledby="off-title">
            <h2 id="off-title">앱이 꺼져 있던 시간</h2>
            <p>{appOffText(w.appOff)}</p>
            <p className="hint">절전과 앱 종료는 집중 점수에서 빼고 따로 보여줍니다.</p>
          </section>
          <section className="card" aria-labelledby="release-title">
            <h2 id="release-title">해제 사용 기록</h2>
            <p>{releaseText(w.releases)}</p>
          </section>
        </>
      )}
    </Loaded>
  );
}

/** Opens on today; the week and weakness views load only when chosen. */
export function DashboardScreen() {
  const [tab, setTab] = useState<Tab>('today');
  return (
    <main className="page">
      <div className="row" role="group" aria-label="대시보드 보기">
        {(Object.keys(TAB_LABEL) as Tab[]).map((key) => (
          <button key={key} type="button" aria-pressed={tab === key} onClick={() => setTab(key)}>
            {TAB_LABEL[key]}
          </button>
        ))}
      </div>
      {tab === 'today' && <CharacterBadge />}
      {tab === 'today' && <TodayView />}
      {tab === 'week' && <WeekView />}
      {tab === 'weakness' && <WeaknessView />}
      {tab === 'growth' && <GrowthTab />}
    </main>
  );
}
