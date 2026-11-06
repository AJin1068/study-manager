import { useEffect, useState } from 'react';
import { commands, type StudyLevel } from '../api';
import { useStudyLock, type LockView } from '../hooks/useStudyLock';
import { useAsync } from '../hooks/useAsync';
import { asyncStatus } from '../hooks/useAsyncStatus';
import { useTimerState } from '../hooks/useTimerState';
import { TimerPanel } from '../components/TimerPanel';
import { FocusSignalsPanel } from '../components/FocusSignalsPanel';
import { TopicPanel } from '../components/TopicPanel';
import { ModeShiftToast } from '../components/ModeShiftToast';
import { failureNotice, modeChangeNotice, type ModeNotice } from './modeShiftNotice';

const LEVEL_LABEL: Record<StudyLevel, string> = {
  easy: '쉬움',
  medium: '중간',
  hard: '어려움',
};

/**
 * Status line for the study card. Precedence: load error, then loading, then the known state.
 * An error is never shown as "loading" or as "lock free".
 */
export function lockStatusText(lock: Pick<LockView, 'loaded' | 'lockHeld' | 'error'>): string {
  if (lock.error !== null) return `상태를 읽지 못했습니다: ${lock.error}`;
  if (!lock.loaded) return '불러오는 중';
  return lock.lockHeld === true ? '잠금 중(계획 진행)' : '공부 외 시간';
}

export function MainScreen() {
  const lock = useStudyLock();
  const modes = useAsync(commands.getModeConfig);
  const timer = useTimerState();
  const timerLockHeld = timer.state === null ? null : timer.state.lockHeld;
  const { refresh: refreshLock } = lock;
  useEffect(() => {
    // The core's lock changed (a run started or ended): re-read it.
    if (timerLockHeld !== null) refreshLock();
  }, [timerLockHeld, refreshLock]);
  const [notice, setNotice] = useState<ModeNotice | null>(null);

  async function chooseMode(level: StudyLevel) {
    try {
      setNotice(modeChangeNotice(await commands.setDailyMode(level)));
      lock.refresh();
      modes.reload();
    } catch (err) {
      setNotice(failureNotice(err));
      lock.refresh();
    }
  }

  const modeStatus = asyncStatus(modes.data, modes.error);
  const snapshot = modeStatus.kind === 'ready' ? modeStatus.value : null;

  return (
    <main className="page">
      <section className="card">
        <h2>공부 상태</h2>
        <p>{lockStatusText(lock)}</p>
        <TimerPanel timer={timer} />
      </section>

      <FocusSignalsPanel timer={timer} />

      <TopicPanel />

      <section className="card" aria-labelledby="mode-title">
        <h2 id="mode-title">공부 모드</h2>
        {modeStatus.kind === 'loading' && <p>불러오는 중</p>}
        {modeStatus.kind === 'error' && modeStatus.stale === null && <p>{modeStatus.message}</p>}
        {modeStatus.kind === 'error' && modeStatus.stale !== null && (
          <p className="error">모드를 새로 읽지 못했습니다: {modeStatus.message}</p>
        )}
        {snapshot && (
          <>
            <p>
              오늘({snapshot.today.date}) : <strong>{LEVEL_LABEL[snapshot.today.level]}</strong>
              {snapshot.today.stored ? '' : ' (기본값, 아직 고르지 않음)'}
            </p>
            <div className="mode-options">
              {snapshot.configs.map((config) => (
                <button
                  key={config.level}
                  type="button"
                  aria-pressed={snapshot.today.level === config.level}
                  disabled={!lock.allowed}
                  onClick={() => void chooseMode(config.level)}
                >
                  <span>{LEVEL_LABEL[config.level]}</span>
                  <small>
                    공부 {config.studyMin}분 / 휴식 {config.breakMin}분
                    {config.longBreakMin !== null && config.setsBeforeLongBreak !== null
                      ? ` (${config.setsBeforeLongBreak}세트마다 긴 휴식 ${config.longBreakMin}분)`
                      : ''}
                  </small>
                </button>
              ))}
            </div>
            {lock.lockHeld === true && <p className="locked">계획 진행 중에는 모드를 바꿀 수 없습니다.</p>}
            {!lock.loaded || lock.error !== null ? (
              <p className="locked">잠금 상태를 확인할 수 없어 모드를 바꿀 수 없습니다.</p>
            ) : null}
          </>
        )}
      </section>

      {notice && (
        <ModeShiftToast
          notice={notice}
          onNotice={setNotice}
          onUndoAttempted={() => {
            lock.refresh();
            modes.reload();
          }}
        />
      )}
    </main>
  );
}
