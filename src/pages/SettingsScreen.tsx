import { useEffect, useState } from 'react';
import {
  commands,
  onResumed,
  type ResumedPayload,
  type SettingKey,
} from '../api/commands';
import { describeOutcome, useStudyLock } from '../hooks/useStudyLock';
import { useAsync } from '../hooks/useAsync';
import { asyncStatus, type AsyncStatus } from '../hooks/useAsyncStatus';

const LOCK_REASON = '공부 중이라 잠겨 있습니다. 공부 외 시간에 바꿀 수 있습니다.';

function formatTime(ms: number): string {
  return new Date(ms).toLocaleString('ko-KR');
}

/** One error line for a read that failed. Stale data never hides the failure. */
function readError<T>(status: AsyncStatus<T>, label: string) {
  return status.kind === 'error' ? <p className="error">{label}: {status.message}</p> : null;
}

export function SettingsScreen() {
  const lock = useStudyLock();
  const settings = useAsync(commands.getSettings);
  const autostartOs = useAsync(commands.getAutostartStatus);
  const power = useAsync(commands.getPowerStatus);
  const history = useAsync(commands.getSettingHistory);
  const { reload: reloadDowntime, ...downtime } = useAsync(commands.getDowntimeRecords);
  const [resumed, setResumed] = useState<ResumedPayload | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onResumed((payload) => {
      setResumed(payload);
      reloadDowntime();
    }).then((stop) => {
      if (disposed) {
        stop();
      } else {
        unlisten = stop;
      }
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [reloadDowntime]);

  async function change(key: SettingKey, value: boolean) {
    try {
      const outcome = await commands.updateSetting(key, value);
      setMessage(describeOutcome(outcome));
      lock.refresh();
    } catch (err) {
      setMessage(`실패: ${String(err)}`);
      // A failed command may mean the lock itself changed; re-read it so controls do not stay enabled.
      lock.refresh();
    }
    settings.reload();
    autostartOs.reload();
    history.reload();
  }

  const settingsStatus = asyncStatus(settings.data, settings.error);
  const autostartStatus = asyncStatus(autostartOs.data, autostartOs.error);
  const powerStatus = asyncStatus(power.data, power.error);
  const current = settingsStatus.kind === 'ready' ? settingsStatus.value : null;

  return (
    <main className="page">
      {lock.studying && <p className="locked" role="note">{LOCK_REASON}</p>}
      {lock.error !== null && <p className="error" role="alert">잠금 상태를 읽지 못해 설정을 바꿀 수 없습니다: {lock.error}</p>}

      <section className="card" aria-labelledby="general-title">
        <h2 id="general-title">일반</h2>
        {readError(settingsStatus, '설정을 읽지 못했습니다')}
        {settingsStatus.kind === 'loading' && <p>불러오는 중</p>}
        {current && (
          <>
            <label className="row">
              <input
                type="checkbox"
                checked={current.autostart}
                onChange={(e) => void change('autostart', e.target.checked)}
              />
              로그인 시 자동 실행
            </label>
            <p className="hint">
              OS 등록 상태:{' '}
              {autostartStatus.kind === 'ready'
                ? autostartStatus.value.enabled ? '등록됨' : '해제됨'
                : autostartStatus.kind === 'error' ? '확인 실패' : '확인 중'}
            </p>
            {readError(autostartStatus, 'OS 등록 상태를 읽지 못했습니다')}

            <label className="row">
              <input
                type="checkbox"
                checked={current.forceQuitAllowed}
                onChange={(e) => void change('forceQuitAllowed', e.target.checked)}
              />
              강제 종료 허용
            </label>
            <p className="hint">
              체크 내용은 기록만 합니다. 실제 강제 종료 집행은 1단계-3에서 붙습니다.
            </p>
          </>
        )}
      </section>

      <section className="card" aria-labelledby="sleep-title">
        <h2 id="sleep-title">수면과 꺼진 시간</h2>
        {readError(powerStatus, '전원 상태를 읽지 못했습니다')}
        {powerStatus.kind === 'ready' && !powerStatus.value.active && (
          <p className="warn">
            전원 이벤트 미지원: {powerStatus.value.reason ?? '사유 없음'}. 하트비트 공백으로만 감지합니다 (검증 안 됨).
          </p>
        )}
        {resumed && (
          <p>
            마지막 재개 {formatTime(resumed.startAtMs)} ~ {formatTime(resumed.endAtMs)}
            {resumed.estimated ? ' (추정)' : ''}. 계획 재확인 대상입니다. 조회는 1단계-2에서 붙습니다.
          </p>
        )}
        <ul className="list">
          {(downtime.data ?? []).slice(0, 10).map((record) => (
            <li key={record.id}>
              {record.kind === 'sleep' ? '수면' : '강제 종료'} {formatTime(record.startAtMs)} ~{' '}
              {formatTime(record.endAtMs)} {record.estimated ? '(추정)' : ''}
            </li>
          ))}
        </ul>
        {downtime.error && <p className="error">{downtime.error}</p>}
      </section>

      <section className="card" aria-labelledby="history-title">
        <h2 id="history-title">변경 기록</h2>
        <ul className="list">
          {(history.data ?? []).slice(0, 50).map((entry) => (
            <li key={entry.id}>
              {formatTime(entry.atMs)} · {entry.key} · {entry.before ?? '없음'} → {entry.after} ·{' '}
              {entry.reason} · {entry.outcome}
            </li>
          ))}
        </ul>
        {history.error && <p className="error">{history.error}</p>}
      </section>

      {message && <p role="status" className="message">{message}</p>}
    </main>
  );
}
