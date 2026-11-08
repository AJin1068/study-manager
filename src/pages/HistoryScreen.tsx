import { useCallback } from 'react';
import { commands } from '../api';
import { useAsync } from '../hooks/useAsync';
import { PlanHistoryPanel } from '../components/PlanHistoryPanel';
import { monthKey } from './calendarLogic';

function localToday(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** Every plan change, newest first, under this month's postponement stats. Read-only. */
export function HistoryScreen() {
  const month = monthKey(localToday());
  const loadStats = useCallback(() => commands.getPlanStats(month), [month]);
  const stats = useAsync(loadStats);
  const s = stats.data;
  const top = s !== null && s.bySubject.length > 0 ? [...s.bySubject].sort((a, b) => b.count - a.count)[0] : null;
  return (
    <main className="page">
      <header>
        <h1>계획 히스토리</h1>
        <p className="sub">
          {s === null
            ? '계획을 바꾼 기록이 여기에 쌓여요'
            : `이번 달 밀림 ${s.postponedCount}회${top !== null ? ` · 가장 자주 밀린 과목: ${top.subject}` : ''}`}
        </p>
        {stats.error !== null && <p className="error">통계를 읽지 못했습니다: {stats.error}</p>}
      </header>
      <PlanHistoryPanel planId={null} month={month} refreshKey={0} hideStats />
    </main>
  );
}
