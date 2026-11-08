import { useCallback, useEffect, useMemo, useState } from 'react';
import { commands, onPlansChanged, type Occurrence } from '../api';
import { ChatPanel } from '../components/ChatPanel';
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

const ROW_PX = 44;

function localToday(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

function minutesOf(time: string): number {
  const [h, m] = time.split(':').map(Number);
  return (h ?? 0) * 60 + (m ?? 0);
}

/** Hours shown by the week grid: every plan fits, at least five rows, evening when empty. */
export function hourRange(items: Pick<Occurrence, 'startTime' | 'durationMin'>[]): number[] {
  let first = 18;
  let last = 23;
  if (items.length > 0) {
    first = Math.min(...items.map((o) => Math.floor(minutesOf(o.startTime) / 60)));
    last = Math.max(...items.map((o) => Math.ceil((minutesOf(o.startTime) + o.durationMin) / 60)));
  }
  first = Math.max(0, Math.min(first, 23));
  last = Math.min(24, Math.max(last, first + 5));
  return Array.from({ length: last - first }, (_, i) => first + i);
}

function dayLabel(date: string, index: number): string {
  return `${WEEKDAY_LABELS[index] ?? ''} ${Number(date.slice(8))}`;
}

function WeekGrid(props: {
  dates: string[];
  today: string;
  byDate: Map<string, Occurrence[]>;
  selected: Occurrence | null;
  onSelect: (o: Occurrence) => void;
  onCreate: (date: string) => void;
}) {
  const all = props.dates.flatMap((d) => props.byDate.get(d) ?? []);
  const hours = hourRange(all);
  const first = hours[0] ?? 0;
  return (
    <div className="calwrap">
      <div className="cal" role="grid" aria-label="주간 계획">
        <div className="h" />
        {props.dates.map((date, i) => (
          <div key={date} role="columnheader" className={`h${date === props.today ? ' today' : ''}`}>
            {dayLabel(date, i)}
          </div>
        ))}
        {hours.map((hour) => (
          <div key={hour} className="cal-row" role="row">
            <div className="t">{String(hour).padStart(2, '0')}:00</div>
            {props.dates.map((date) => {
              const starting = (props.byDate.get(date) ?? []).filter((o) => {
                const startHour = Math.floor(minutesOf(o.startTime) / 60);
                return startHour === hour || (hour === first && startHour < first);
              });
              return (
                <div
                  key={date}
                  role="gridcell"
                  className={`c${date === props.today ? ' today' : ''}`}
                  onDoubleClick={() => props.onCreate(date)}
                >
                  {starting.map((o) => {
                    const offset = Math.max(0, minutesOf(o.startTime) - hour * 60);
                    const top = 3 + (offset / 60) * ROW_PX;
                    const height = Math.max(22, (o.durationMin / 60) * ROW_PX - 6);
                    const isSel = props.selected?.planId === o.planId && props.selected?.sourceDate === o.sourceDate;
                    return (
                      <button
                        key={`${o.planId}-${o.sourceDate}`}
                        type="button"
                        className={`ev ${o.moved ? 'd' : 's'}${isSel ? ' sel' : ''}`}
                        style={{ top, height }}
                        aria-pressed={isSel}
                        onClick={() => props.onSelect(o)}
                      >
                        <b>{o.name}</b>
                        <small>
                          {derivedEnd(o.startTime, o.durationMin)}
                          {o.moved ? ' · 옮김' : ''}
                        </small>
                      </button>
                    );
                  })}
                </div>
              );
            })}
          </div>
        ))}
      </div>
    </div>
  );
}

function MonthGrid(props: {
  weeks: string[][];
  anchor: string;
  today: string;
  byDate: Map<string, Occurrence[]>;
  onSelect: (o: Occurrence) => void;
  onCreate: (date: string) => void;
}) {
  return (
    <div className="calwrap">
      <div className="cal-grid" role="grid" aria-label="월간 계획">
        {WEEKDAY_LABELS.map((label) => (
          <div key={label} className="cal-head" role="columnheader">
            {label}
          </div>
        ))}
        {props.weeks.flat().map((date) => {
          const items = props.byDate.get(date) ?? [];
          const outside = monthKey(date) !== monthKey(props.anchor);
          return (
            <div key={date} role="gridcell" className={`cal-cell${date === props.today ? ' today' : ''}${outside ? ' outside' : ''}`}>
              <button type="button" className="cal-day" onClick={() => props.onCreate(date)} aria-label={`${date}에 계획 추가`}>
                {Number(date.slice(8))}
              </button>
              {items.map((o) => (
                <button
                  key={`${o.planId}-${o.sourceDate}`}
                  type="button"
                  className={`cal-item${o.moved ? ' moved' : ''}`}
                  onClick={() => props.onSelect(o)}
                >
                  {o.startTime} {o.name}
                </button>
              ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}

export function CalendarScreen() {
  const [today] = useState(localToday);
  const [view, setView] = useState<CalendarView>('week');
  const [anchor, setAnchor] = useState(today);
  const [occurrences, setOccurrences] = useState<Occurrence[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [dialog, setDialog] = useState<DialogTarget | null>(null);
  const [selected, setSelected] = useState<Occurrence | null>(null);
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
  const rangeText = view === 'week' ? `${from.slice(5).replace('-', '/')} ~ ${to.slice(5).replace('-', '/')}` : monthKey(anchor);

  return (
    <main className="page wide">
      <header className="row space-between page-head">
        <div>
          <h1>캘린더</h1>
          <p className="sub">{rangeText} · 계획을 옮기거나 지우면 히스토리에 기록돼요</p>
        </div>
        <button type="button" className="pri" onClick={() => setDialog({ kind: 'create', date: today })}>
          + 공부 계획
        </button>
      </header>

      <div className="row toolbar">
        <div className="seg" role="group" aria-label="이동">
          <button type="button" onClick={() => setAnchor(shiftAnchor(view, anchor, -1))} aria-label="이전">
            ‹
          </button>
          <button type="button" onClick={() => setAnchor(today)}>
            오늘
          </button>
          <button type="button" onClick={() => setAnchor(shiftAnchor(view, anchor, 1))} aria-label="다음">
            ›
          </button>
        </div>
        <div className="seg" role="group" aria-label="보기">
          <button type="button" aria-pressed={view === 'week'} onClick={() => setView('week')}>
            주
          </button>
          <button type="button" aria-pressed={view === 'month'} onClick={() => setView('month')}>
            월
          </button>
        </div>
      </div>
      {loadError !== null && <p className="error">계획을 읽지 못했습니다: {loadError}</p>}

      {view === 'week' ? (
        <WeekGrid
          dates={weekDates(anchor)}
          today={today}
          byDate={byDate}
          selected={selected}
          onSelect={setSelected}
          onCreate={(date) => setDialog({ kind: 'create', date })}
        />
      ) : (
        <MonthGrid
          weeks={monthGrid(anchor)}
          anchor={anchor}
          today={today}
          byDate={byDate}
          onSelect={setSelected}
          onCreate={(date) => setDialog({ kind: 'create', date })}
        />
      )}
      <div className="legend">
        <span className="pill p-study">공부 계획</span>
        <span className="pill p-break">옮겨진 계획</span>
        <span>{view === 'week' ? '빈 칸을 두 번 누르면 그날 계획을 추가해요' : '날짜를 누르면 그날 계획을 추가해요'}</span>
      </div>

      {selected !== null && (
        <section className="card selected-plan" aria-label="선택한 계획">
          <div className="row space-between">
            <div>
              <b>{selected.name}</b>{' '}
              <span className="hint">
                {selected.subject} · {selected.date} {derivedEnd(selected.startTime, selected.durationMin)} ·{' '}
                {selected.sets}세트{selected.moved ? ' · 옮김' : ''}
              </span>
            </div>
            <div className="row">
              <button type="button" onClick={() => setDialog({ kind: 'edit', occurrence: selected })}>
                고치기
              </button>
              <button type="button" onClick={() => setDialog({ kind: 'delete', occurrence: selected })}>
                지우기
              </button>
              <button type="button" onClick={() => setSelected(null)}>
                닫기
              </button>
            </div>
          </div>
        </section>
      )}

      {dialog && (
        <PlanDialog
          key={JSON.stringify(dialog)}
          target={dialog}
          onClose={() => setDialog(null)}
          onResult={(change) => {
            setNotice(planChangeMessage(change));
            setRefreshKey((k) => k + 1);
            setSelected(null);
            reload();
          }}
        />
      )}
      {notice && (
        <p role="status" className={notice.rejected ? 'message locked' : 'message'}>
          {notice.text}
        </p>
      )}

      <div className="grid g-mid">
        <PlanHistoryPanel planId={selected?.planId ?? null} month={monthKey(anchor)} refreshKey={refreshKey} />
        <ChatPanel />
      </div>
    </main>
  );
}
