import { useState } from 'react';
import { MainScreen } from './pages/MainScreen';
import { SettingsScreen } from './pages/SettingsScreen';

type Page = 'main' | 'settings';

export function App() {
  const [page, setPage] = useState<Page>('main');

  return (
    <div className="app">
      <nav className="tabs" aria-label="화면 전환">
        <button type="button" aria-pressed={page === 'main'} onClick={() => setPage('main')}>
          메인
        </button>
        <button type="button" aria-pressed={page === 'settings'} onClick={() => setPage('settings')}>
          설정
        </button>
      </nav>
      {page === 'main' ? <MainScreen /> : <SettingsScreen />}
    </div>
  );
}
