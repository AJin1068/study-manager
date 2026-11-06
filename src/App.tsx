import { useState } from 'react';
import { CheerToastHost } from './components/CheerToastHost';
import { FirstRunCharacter } from './components/FirstRunCharacter';
import { FirstRunPresets } from './components/FirstRunPresets';
import { QuizCardHost } from './components/QuizCardHost';
import { CalendarScreen } from './pages/CalendarScreen';
import { CharacterScreen } from './pages/CharacterScreen';
import { DashboardScreen } from './pages/DashboardScreen';
import { MainScreen } from './pages/MainScreen';
import { ProfileScreen } from './pages/ProfileScreen';
import { RoomsScreen } from './pages/RoomsScreen';
import { SettingsScreen } from './pages/SettingsScreen';
import { WikiScreen } from './pages/WikiScreen';

type Page = 'main' | 'calendar' | 'dashboard' | 'wiki' | 'rooms' | 'profile' | 'character' | 'settings';

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
        <button type="button" aria-pressed={page === 'wiki'} onClick={() => setPage('wiki')}>
          위키
        </button>
        <button type="button" aria-pressed={page === 'rooms'} onClick={() => setPage('rooms')}>
          스터디방
        </button>
        <button type="button" aria-pressed={page === 'profile'} onClick={() => setPage('profile')}>
          프로필
        </button>
        <button type="button" aria-pressed={page === 'character'} onClick={() => setPage('character')}>
          캐릭터
        </button>
        <button type="button" aria-pressed={page === 'settings'} onClick={() => setPage('settings')}>
          설정
        </button>
      </nav>
      <FirstRunPresets />
      <FirstRunCharacter />
      <QuizCardHost />
      <CheerToastHost />
      {page === 'main' && <MainScreen />}
      {page === 'calendar' && <CalendarScreen />}
      {page === 'dashboard' && <DashboardScreen />}
      {page === 'wiki' && <WikiScreen />}
      {page === 'rooms' && <RoomsScreen />}
      {page === 'profile' && <ProfileScreen />}
      {page === 'character' && <CharacterScreen />}
      {page === 'settings' && <SettingsScreen />}
    </div>
  );
}
