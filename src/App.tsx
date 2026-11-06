import { useState } from 'react';
import { FirstRunPresets } from './components/FirstRunPresets';
import { CalendarScreen } from './pages/CalendarScreen';
import { DashboardScreen } from './pages/DashboardScreen';
import { MainScreen } from './pages/MainScreen';
import { SettingsScreen } from './pages/SettingsScreen';

type Page = 'main' | 'calendar' | 'dashboard' | 'settings';

export function App() {
  const [page, setPage] = useState<Page>('main');

  return (
    <div className="app">
      <nav className="tabs" aria-label="화면 전환">
        <button type="button" aria-pressed={page === 'main'} onClick={() => setPage('main')}>
          메인
        </button>
        <button type="button" aria-pressed={page === 'calendar'} onClick={() => setPage('calendar')}>
          계획
        </button>
        <button type="button" aria-pressed={page === 'dashboard'} onClick={() => setPage('dashboard')}>
          대시보드
        </button>
        <button type="button" aria-pressed={page === 'settings'} onClick={() => setPage('settings')}>
          설정
        </button>
      </nav>
      <FirstRunPresets />
      {page === 'main' && <MainScreen />}
      {page === 'calendar' && <CalendarScreen />}
      {page === 'dashboard' && <DashboardScreen />}
      {page === 'settings' && <SettingsScreen />}
    </div>
  );
}
