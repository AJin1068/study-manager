import { useCallback, useEffect } from 'react';
import { commands, onPlansChanged, type PlanHistoryEntry } from '../api';
import { useAsync } from '../hooks/useAsync';
import { describeHistoryEntry, formatAt, reasonLabel } from '../pages/calendarLogic';

const HISTORY_LIMIT = 100;

interface Props {
  /** A plan id shows that plan's series timeline; null shows everything. */
  planId: number | null;
  month: string;
  /** Bumped by the parent after its own changes so both lists refetch together. */
  refreshKey: number;
}

function HistoryRow({ entry }: { entry: PlanHistoryEntry }) {
  return (
    <li className={entry.outcome === 'rejected_locked' ? 'locked' : undefined}>
      {formatAt(entry.atMs)} · {describeHistoryEntry(entry)} · {reasonLabel(entry.reasonCode)}
      {entry.reasonText ? ` (${entry.reasonText})` : ''}
    </li>
  );
}

export function PlanHistoryPanel({ planId, month, refreshKey }: Props) {
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
    <section className="card" aria-labelledby="history-title">
      <h2 id="history-title">수정 히스토리</h2>
      {stats.error !== null && <p className="error">통계를 읽지 못했습니다: {stats.error}</p>}
      {stats.data && (
        <p>
          {stats.data.month} 밀린 횟수: <strong>{stats.data.postponedCount}회</strong>
          {stats.data.bySubject.length > 0 && (
            <>
              {' '}· 자주 밀리는 과목:{' '}
              {stats.data.bySubject.map((s) => `${s.subject} ${s.count}회`).join(', ')}
            </>
          )}
        </p>
      )}
      <p className="hint">{planId === null ? '전체 기록' : '선택한 계획의 기록'}</p>
      {history.error !== null && <p className="error">히스토리를 읽지 못했습니다: {history.error}</p>}
      {history.data === null && history.error === null && <p>불러오는 중</p>}
      {history.data && history.data.length === 0 && <p>기록이 없습니다.</p>}
      {history.data && history.data.length > 0 && (
        <ul className="list">
          {history.data.map((entry) => (
            <HistoryRow key={entry.id} entry={entry} />
          ))}
        </ul>
      )}
    </section>
  );
}
