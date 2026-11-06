import { useCallback, useEffect, useMemo, useState } from 'react';
import { commands, onPlansChanged, type Occurrence } from '../api';
import { PlanDialog, type DialogTarget } from '../components/PlanDialog';
import { PlanHistoryPanel } from '../components/PlanHistoryPanel';
import {
  WEEKDAY_LABELS,
  derivedEnd,
  groupByDate,
  monthGrid,
  monthKey,
  planChangeMessage,
  shiftAnchor,
  visibleRange,
  weekDates,
  type CalendarView,
} from './calendarLogic';

function localToday(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

export function CalendarScreen() {
  const [today] = useState(localToday);
  const [view, setView] = useState<CalendarView>('week');
  const [anchor, setAnchor] = useState(today);
  const [occurrences, setOccurrences] = useState<Occurrence[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [dialog, setDialog] = useState<DialogTarget | null>(null);
  const [selectedPlan, setSelectedPlan] = useState<number | null>(null);
  const [notice, setNotice] = useState<{ text: string; rejected: boolean } | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);

  const { from, to } = useMemo(() => visibleRange(view, anchor), [view, anchor]);

  const reload = useCallback(() => {
    commands
      .listOccurrences(from, to)
      .then((list) => {
        setOccurrences(list);
        setLoadError(null);
      })
      .catch((err: unknown) => setLoadError(String(err)));
  }, [from, to]);

  useEffect(() => {
    reload();
  }, [reload]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let active = true;
    void onPlansChanged(reload).then((fn) => {
      if (active) unlisten = fn;
      else fn();
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [reload]);

  const byDate = useMemo(() => groupByDate(occurrences), [occurrences]);
  const weeks = view === 'week' ? [weekDates(anchor)] : monthGrid(anchor);

  return (
    <main className="page wide">
      <section className="card" aria-labelledby="calendar-title">
        <h2 id="calendar-title">계획 캘린더</h2>
        <div className="row">
          <button type="button" onClick={() => setAnchor(shiftAnchor(view, anchor, -1))}>이전</button>
          <button type="button" onClick={() => setAnchor(today)}>오늘</button>
          <button type="button" onClick={() => setAnchor(shiftAnchor(view, anchor, 1))}>다음</button>
          <strong>{view === 'week' ? `${from} ~ ${to}` : monthKey(anchor)}</strong>
          <button type="button" aria-pressed={view === 'week'} onClick={() => setView('week')}>주</button>
          <button type="button" aria-pressed={view === 'month'} onClick={() => setView('month')}>월</button>
          <button type="button" onClick={() => setDialog({ kind: 'create', date: today })}>계획 추가</button>
        </div>
        {loadError !== null && <p className="error">계획을 읽지 못했습니다: {loadError}</p>}

        <div className="cal-grid" role="grid" aria-label="계획 캘린더">
          {WEEKDAY_LABELS.map((label) => (
            <div key={label} className="cal-head" role="columnheader">{label}</div>
          ))}
          {weeks.flat().map((date) => {
            const items = byDate.get(date) ?? [];
            const outside = view === 'month' && monthKey(date) !== monthKey(anchor);
            return (
              <div
                key={date}
                role="gridcell"
                className={`cal-cell${date === today ? ' today' : ''}${outside ? ' outside' : ''}`}
              >
                <button type="button" className="cal-day" onClick={() => setDialog({ kind: 'create', date })}>
                  {date.slice(8)}
                </button>
                {items.map((o) => (
                  <div key={`${o.planId}-${o.sourceDate}`} className="cal-item">
                    <button type="button" onClick={() => setSelectedPlan(o.planId)}>
                      {o.startTime} {o.name}
                    </button>
                    <small>
                      {o.subject} · {derivedEnd(o.startTime, o.durationMin)}
                      {o.moved ? ' · 옮김' : ''}
                    </small>
                    <span className="row">
                      <button type="button" onClick={() => setDialog({ kind: 'edit', occurrence: o })}>고치기</button>
                      <button type="button" onClick={() => setDialog({ kind: 'delete', occurrence: o })}>지우기</button>
                    </span>
                  </div>
                ))}
              </div>
            );
          })}
        </div>
      </section>

      {dialog && (
        <PlanDialog
          key={JSON.stringify(dialog)}
          target={dialog}
          onClose={() => setDialog(null)}
          onResult={(change) => {
            setNotice(planChangeMessage(change));
            setRefreshKey((k) => k + 1);
            reload();
          }}
        />
      )}
      {notice && (
        <p role="status" className={notice.rejected ? 'message locked' : 'message'}>{notice.text}</p>
      )}

      <div className="row">
        <button type="button" aria-pressed={selectedPlan === null} onClick={() => setSelectedPlan(null)}>
          전체 기록 보기
        </button>
      </div>
      <PlanHistoryPanel planId={selectedPlan} month={monthKey(anchor)} refreshKey={refreshKey} />
    </main>
  );
}
