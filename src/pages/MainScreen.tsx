import { useState } from 'react';
import { commands, type StudyLevel } from '../api/commands';
import { describeOutcome, useStudyLock, type LockView } from '../hooks/useStudyLock';
import { useAsync } from '../hooks/useAsync';
import { asyncStatus } from '../hooks/useAsyncStatus';

const LEVEL_LABEL: Record<StudyLevel, string> = {
  easy: '쉬움',
  medium: '중간',
  hard: '어려움',
};

/**
 * Status line for the study card. Precedence: load error, then loading, then the known state.
 * An error is never shown as "loading" or as "not studying".
 */
export function lockStatusText(lock: Pick<LockView, 'loaded' | 'studying' | 'error'>): string {
  if (lock.error !== null) return `상태를 읽지 못했습니다: ${lock.error}`;
  if (!lock.loaded) return '불러오는 중';
  return lock.studying === true ? '공부 중 (설정 잠금)' : '공부 외 시간';
}

export function MainScreen() {
  const lock = useStudyLock();
  const modes = useAsync(commands.getModeConfig);
  const [message, setMessage] = useState<string | null>(null);

  async function toggleStudy() {
    // The button is disabled until the state loads, so `studying` is a real boolean here.
    const next = lock.studying !== true;
    try {
      await commands.setStudyStateTest(next);
      lock.refresh();
      setMessage(next ? '공부 모드를 켰습니다. 설정이 잠깁니다.' : '공부 모드를 껐습니다.');
    } catch (err) {
      setMessage(`실패: ${String(err)}`);
      lock.refresh();
    }
  }

  async function chooseMode(level: StudyLevel) {
    try {
      const outcome = await commands.setDailyMode(level);
      setMessage(describeOutcome(outcome));
      lock.refresh();
      modes.reload();
    } catch (err) {
      setMessage(`실패: ${String(err)}`);
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
        <button type="button" onClick={() => void toggleStudy()} disabled={!lock.loaded}>
          {lock.studying === true ? '공부 끝내기' : '공부 시작 (테스트 토글)'}
        </button>
        <p className="hint">테스트용 토글입니다. 자동 타이머는 1단계-2에서 붙습니다.</p>
      </section>

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
            {lock.studying === true && <p className="locked">공부 중에는 모드를 바꿀 수 없습니다.</p>}
            {!lock.loaded || lock.error !== null ? (
              <p className="locked">잠금 상태를 확인할 수 없어 모드를 바꿀 수 없습니다.</p>
            ) : null}
          </>
        )}
      </section>

      {message && <p role="status" className="message">{message}</p>}
    </main>
  );
}
