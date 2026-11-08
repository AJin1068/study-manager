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
import { MainScreen } from './pages/MainScreen';
import { ProfileScreen } from './pages/ProfileScreen';
import { RoomsScreen } from './pages/RoomsScreen';
import { SettingsScreen } from './pages/SettingsScreen';
import { WikiScreen } from './pages/WikiScreen';

type Page = 'main' | 'calendar' | 'history' | 'dashboard' | 'wiki' | 'rooms' | 'profile' | 'character' | 'settings';

const PAGES: { page: Page; label: string; sub: string }[] = [
  { page: 'main', label: '메인', sub: '지금 공부 상태와 오늘의 모드' },
  { page: 'calendar', label: '캘린더', sub: '계획을 만들고 옮기면 히스토리에 기록돼요' },
  { page: 'history', label: '계획 히스토리', sub: '계획을 바꾼 기록과 자주 밀리는 과목' },
  { page: 'dashboard', label: '대시보드', sub: '계획 대비 실제, 약점, 성장' },
  { page: 'wiki', label: '위키', sub: '과목별 정리와 복습 문제' },
  { page: 'rooms', label: '스터디방', sub: '같이 공부하는 사람들' },
  { page: 'profile', label: '프로필', sub: '내 프로필 홈과 공개 범위' },
  { page: 'character', label: '캐릭터', sub: '캐릭터와 상점, 포인트 내역' },
  { page: 'settings', label: '설정', sub: '바뀐 설정은 모두 히스토리에 남아요' },
];

/** Name, level and balance at the bottom of the sidebar. Hidden until the character exists. */
function SidebarMe() {
  const character = useAsync(commands.getCharacter);
  const points = useAsync(commands.getPointsStatus);
  const view = character.data;
  if (view === null) return null;
  return (
    <div className="me">
      <CharacterAvatar svg={view.svg} label={`${view.name} (${view.stageLabel})`} size={32} />
      <div>
        <b>{view.name}</b>
        <br />
        <small>
          Lv.{view.level}
          {points.data !== null && ` · ${points.data.balance.toLocaleString()}P`}
        </small>
      </div>
    </div>
  );
}

export function App() {
  const [page, setPage] = useState<Page>('main');
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
        <SidebarMe />
      </nav>
      <div className="content">
        <FirstRunPresets />
        <FirstRunCharacter />
        <QuizCardHost />
        <CheerToastHost />
        <h1>{current.label}</h1>
        <p className="sub">{current.sub}</p>
        {page === 'main' && <MainScreen />}
        {page === 'calendar' && <CalendarScreen />}
        {page === 'history' && <HistoryScreen />}
        {page === 'dashboard' && <DashboardScreen />}
        {page === 'wiki' && <WikiScreen />}
        {page === 'rooms' && <RoomsScreen />}
        {page === 'profile' && <ProfileScreen />}
        {page === 'character' && <CharacterScreen />}
        {page === 'settings' && <SettingsScreen />}
      </div>
    </div>
  );
}
