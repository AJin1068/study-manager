import { useState } from 'react';
import { commands, type ModeSnapshot, type StudyLevel } from '../api';
import type { LockView } from '../hooks/useStudyLock';
import { asyncStatus } from '../hooks/useAsyncStatus';
import { ModeShiftToast } from '../components/ModeShiftToast';
import { failureNotice, modeChangeNotice, type ModeNotice } from './modeShiftNotice';

// The former main screen's pieces. The dashboard (home) now places them; see DashboardScreen.

export const LEVEL_LABEL: Record<StudyLevel, string> = {
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

/** Today's study mode (easy / medium / hard). Changing it is refused while a run holds the lock. */
export function ModeCard(props: {
  lock: LockView & { refresh: () => void };
  modes: { data: ModeSnapshot | null; error: string | null; reload: () => void };
}) {
  const { lock, modes } = props;
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
    <section className="card" aria-labelledby="mode-title">
      <div className="row space-between">
        <h2 id="mode-title">오늘의 공부 모드</h2>
        {snapshot && !snapshot.today.stored && <span className="pill p-plain">기본값</span>}
      </div>
      {modeStatus.kind === 'loading' && <p>불러오는 중</p>}
      {modeStatus.kind === 'error' && modeStatus.stale === null && <p>{modeStatus.message}</p>}
      {modeStatus.kind === 'error' && modeStatus.stale !== null && (
        <p className="error">모드를 새로 읽지 못했습니다: {modeStatus.message}</p>
      )}
      {snapshot && (
        <>
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
                  {config.studyMin}분 / {config.breakMin}분
                  {config.longBreakMin !== null && config.setsBeforeLongBreak !== null
                    ? ` · ${config.setsBeforeLongBreak}세트마다 ${config.longBreakMin}분`
                    : ''}
                </small>
              </button>
            ))}
          </div>
          {lock.lockHeld === true && <p className="hint">계획 진행 중에는 모드를 바꿀 수 없어요.</p>}
          {!lock.loaded || lock.error !== null ? (
            <p className="locked">잠금 상태를 확인할 수 없어 모드를 바꿀 수 없습니다.</p>
          ) : null}
        </>
      )}

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
    </section>
  );
}
