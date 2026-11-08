import { useCallback, useEffect } from 'react';
import { commands, onPlansChanged, type PlanHistoryEntry } from '../api';
import { useAsync } from '../hooks/useAsync';
import { formatAt, historyParts, reasonLabel } from '../pages/calendarLogic';

const HISTORY_LIMIT = 100;

interface Props {
  /** A plan id shows that plan's series timeline; null shows everything. */
  planId: number | null;
  month: string;
  /** Bumped by the parent after its own changes so both lists refetch together. */
  refreshKey: number;
  /** The history screen shows the stats in its own heading. */
  hideStats?: boolean;
}

/** Moves and deletions are the "밀림 / 취소" the mockup highlights. */
function tag(entry: PlanHistoryEntry): { text: string; className: string } | null {
  if (entry.outcome === 'rejected_locked') return { text: '거절', className: 'p-bad' };
  if (entry.action === 'delete') return { text: '취소', className: 'p-bad' };
  if (entry.action === 'move' && entry.reasonCode !== 'created') return { text: '밀림', className: 'p-break' };
  return null;
}

function HistoryRow({ entry }: { entry: PlanHistoryEntry }) {
  const parts = historyParts(entry);
  const pill = tag(entry);
  return (
    <li className={pill !== null ? 'tl-warn' : undefined}>
      <div className="when">{formatAt(entry.atMs)}</div>
      <b>{parts.head}</b> {pill !== null && <span className={`pill ${pill.className}`}>{pill.text}</span>}
      {(parts.before !== '' || parts.after !== '') && (
        <div className="diff">
          {parts.before !== '' && <s>{parts.before}</s>}
          {parts.before !== '' && parts.after !== '' && <span aria-hidden="true">→</span>}
          {parts.after !== '' && <span>{parts.after}</span>}
        </div>
      )}
      {(reasonLabel(entry.reasonCode) !== '' || entry.reasonText) && (
        <div className="hint">
          이유: {reasonLabel(entry.reasonCode)}
          {entry.reasonText ? ` (${entry.reasonText})` : ''}
        </div>
      )}
    </li>
  );
}

export function PlanHistoryPanel({ planId, month, refreshKey, hideStats = false }: Props) {
  const loadHistory = useCallback(
    () => commands.getPlanHistory(planId ?? undefined, HISTORY_LIMIT),
    [planId],
  );
  const loadStats = useCallback(() => commands.getPlanStats(month), [month]);
  const history = useAsync(loadHistory);
  const stats = useAsync(loadStats);
  const { reload: reloadHistory } = history;
  const { reload: reloadStats } = stats;

  useEffect(() => {
    reloadHistory();
    reloadStats();
  }, [refreshKey, reloadHistory, reloadStats]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let active = true;
    void onPlansChanged(() => {
      reloadHistory();
      reloadStats();
    }).then((fn) => {
      if (active) unlisten = fn;
      else fn();
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [reloadHistory, reloadStats]);

  return (
    <section className={hideStats ? 'timeline' : 'card'} aria-labelledby="history-title">
      <h2 id="history-title" className={hideStats ? 'visually-hidden' : undefined}>
        {planId === null ? '수정 히스토리' : '선택한 계획의 수정 히스토리'}
      </h2>
      {!hideStats && stats.error !== null && <p className="error">통계를 읽지 못했습니다: {stats.error}</p>}
      {!hideStats && stats.data && (
        <p className="hint">
          {stats.data.month} 밀린 횟수 {stats.data.postponedCount}회
          {stats.data.bySubject.length > 0 &&
            ` · 자주 밀리는 과목: ${stats.data.bySubject.map((s) => `${s.subject} ${s.count}회`).join(', ')}`}
        </p>
      )}
      {history.error !== null && <p className="error">히스토리를 읽지 못했습니다: {history.error}</p>}
      {history.data === null && history.error === null && <p className="hint">불러오는 중</p>}
      {history.data && history.data.length === 0 && <p className="hint">아직 계획을 바꾼 기록이 없어요.</p>}
      {history.data && history.data.length > 0 && (
        <ul className="tl">
          {history.data.map((entry) => (
            <HistoryRow key={entry.id} entry={entry} />
          ))}
        </ul>
      )}
    </section>
  );
}
