import { PlanHistoryPanel } from '../components/PlanHistoryPanel';
import { monthKey } from './calendarLogic';

function localToday(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** Every plan change, newest first, with this month's postponement stats. Read-only. */
export function HistoryScreen() {
  return (
    <main className="page">
      <PlanHistoryPanel planId={null} month={monthKey(localToday())} refreshKey={0} />
    </main>
  );
}
