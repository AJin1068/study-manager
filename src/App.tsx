import { useState } from 'react';
import { commands } from './api';
import { CharacterAvatar } from './components/CharacterAvatar';
import { CheerToastHost } from './components/CheerToastHost';
import { FirstRunCharacter } from './components/FirstRunCharacter';
import { FirstRunPresets } from './components/FirstRunPresets';
import { QuizCardHost } from './components/QuizCardHost';
import { useAsync } from './hooks/useAsync';
import { CalendarScreen } from './pages/CalendarScreen';
import { CharacterScreen } from './pages/CharacterScreen';
import { DashboardScreen } from './pages/DashboardScreen';
import { HistoryScreen } from './pages/HistoryScreen';
import { ProfileScreen } from './pages/ProfileScreen';
import { RoomsScreen } from './pages/RoomsScreen';
import { SettingsScreen } from './pages/SettingsScreen';
import { WikiScreen } from './pages/WikiScreen';

type Page = 'dashboard' | 'calendar' | 'history' | 'rooms' | 'character' | 'wiki' | 'profile' | 'settings';

/** Sidebar order follows docs/design/mockup.html; wiki and profile sit after its six screens. */
const PAGES: { page: Page; label: string; sub: string | null }[] = [
  { page: 'dashboard', label: '대시보드', sub: null },
  { page: 'calendar', label: '캘린더', sub: null },
  { page: 'history', label: '계획 히스토리', sub: null },
  { page: 'rooms', label: '스터디방', sub: '같이 공부하는 사람들' },
  { page: 'character', label: '캐릭터', sub: null },
  { page: 'wiki', label: '위키', sub: '과목별 정리와 복습 문제' },
  { page: 'profile', label: '프로필', sub: '내 프로필 홈과 공개 범위' },
  { page: 'settings', label: '설정', sub: '바뀐 설정은 모두 히스토리에 남아요' },
];

/** Name, level and balance at the bottom of the sidebar. Hidden until the character exists. */
function SidebarMe({ onOpen }: { onOpen: () => void }) {
  const character = useAsync(commands.getCharacter);
  const points = useAsync(commands.getPointsStatus);
  const view = character.data;
  if (view === null) return null;
  return (
    <button type="button" className="me" onClick={onOpen} aria-label="내 캐릭터 보기">
      <CharacterAvatar svg={view.svg} label={`${view.name} (${view.stageLabel})`} size={32} />
      <div>
        <b>{view.name}</b>
        <br />
        <small>
          Lv.{view.level}
          {points.data !== null && ` · ${points.data.balance.toLocaleString()}P`}
        </small>
      </div>
    </button>
  );
}

export function App() {
  const [page, setPage] = useState<Page>('dashboard');
  const current = PAGES.find((p) => p.page === page) ?? PAGES[0];

  return (
    <div className="app">
      <nav className="tabs" aria-label="화면 전환">
        <div className="brand">
          <i />
          Study Manager
        </div>
        {PAGES.map((p) => (
          <button key={p.page} type="button" aria-current={page === p.page} onClick={() => setPage(p.page)}>
            {p.label}
          </button>
        ))}
        <div className="spacer" />
        <SidebarMe onOpen={() => setPage('character')} />
      </nav>
      <div className="content">
        <FirstRunPresets />
        <FirstRunCharacter />
        <QuizCardHost />
        <CheerToastHost />
        {current.sub !== null && (
          <>
            <h1>{current.label}</h1>
            <p className="sub">{current.sub}</p>
          </>
        )}
        {page === 'dashboard' && <DashboardScreen />}
        {page === 'calendar' && <CalendarScreen />}
        {page === 'history' && <HistoryScreen />}
        {page === 'rooms' && <RoomsScreen />}
        {page === 'character' && <CharacterScreen />}
        {page === 'wiki' && <WikiScreen />}
        {page === 'profile' && <ProfileScreen />}
        {page === 'settings' && <SettingsScreen />}
      </div>
    </div>
  );
}
