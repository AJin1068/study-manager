import { useState } from 'react';
import { DiagnosticsScreen } from './screens/DiagnosticsScreen';
import { PairScreen } from './screens/PairScreen';
import { ScheduleScreen } from './screens/ScheduleScreen';
import { TimerScreen } from './screens/TimerScreen';
import { usePhoneView } from './usePhoneView';

type Tab = 'timer' | 'schedule' | 'diagnostics' | 'pair';

const TABS: { id: Tab; label: string }[] = [
  { id: 'timer', label: '타이머' },
  { id: 'schedule', label: '시간표' },
  { id: 'diagnostics', label: '진단' },
  { id: 'pair', label: '짝짓기' },
];

export function App() {
  const [tab, setTab] = useState<Tab>('timer');
  const { view, error, refresh } = usePhoneView();

  return (
    <div>
      <nav aria-label="화면 전환">
        {TABS.map(({ id, label }) => (
          <button key={id} type="button" aria-pressed={tab === id} onClick={() => setTab(id)}>
            {label}
          </button>
        ))}
      </nav>
      {error !== null && <p className="warn">상태를 읽지 못했습니다: {error}</p>}
      {tab === 'timer' && <TimerScreen view={view} />}
      {tab === 'schedule' && <ScheduleScreen />}
      {tab === 'diagnostics' && <DiagnosticsScreen view={view} onRefresh={refresh} />}
      {tab === 'pair' && <PairScreen view={view} onChanged={refresh} />}
    </div>
  );
}
