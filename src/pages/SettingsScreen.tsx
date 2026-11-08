import { useEffect, useState } from 'react';
import {
  commands,
  onResumed,
  type ResumedPayload,
  type SettingKey,
  type SettingValue,
} from '../api';
import { describeOutcome, useStudyLock } from '../hooks/useStudyLock';
import { useAsync } from '../hooks/useAsync';
import { asyncStatus, type AsyncStatus } from '../hooks/useAsyncStatus';
import { ModeConfigEditor } from './ModeConfigEditor';
import { BlockListPanel } from '../components/BlockListPanel';
import { CalendarIoPanel } from '../components/CalendarIoPanel';
import { PhoneInstallPanel } from '../components/PhoneInstallPanel';
import { PhonePairPanel } from '../components/PhonePairPanel';
import { PresetPanel } from '../components/PresetPanel';
import { ContextSettingsPanel } from '../components/ContextSettingsPanel';
import { MessageSettingsPanel } from '../components/MessageSettingsPanel';
import { QuizSettingsPanel } from '../components/QuizSettingsPanel';
import { SiteRulesPanel } from '../components/SiteRulesPanel';
import { ModeShiftToast } from '../components/ModeShiftToast';
import { IntegrationsScreen } from './IntegrationsScreen';
import type { ModeNotice } from './modeShiftNotice';

const LOCK_REASON = '지금은 공부 시간이라 설정이 잠겨 있어요. 진행이 끝난 뒤에 바꿀 수 있어요.';

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
  const modes = useAsync(commands.getModeConfig);
  const history = useAsync(commands.getSettingHistory);
  const { reload: reloadDowntime, ...downtime } = useAsync(commands.getDowntimeRecords);
  const [resumed, setResumed] = useState<ResumedPayload | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [modeNotice, setModeNotice] = useState<ModeNotice | null>(null);

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

  async function change(key: SettingKey, value: SettingValue) {
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

  function modeAttempted(notice: ModeNotice) {
    setModeNotice(notice);
    lock.refresh();
    modes.reload();
    history.reload();
  }

  function blockAttempted(text: string) {
    setMessage(text);
    lock.refresh();
    history.reload();
  }

  const settingsStatus = asyncStatus(settings.data, settings.error);
  const autostartStatus = asyncStatus(autostartOs.data, autostartOs.error);
  const powerStatus = asyncStatus(power.data, power.error);
  const current = settingsStatus.kind === 'ready' ? settingsStatus.value : null;

  const groups: { id: string; label: string }[] = [
    { id: 'set-general', label: '일반' },
    { id: 'set-mode', label: '공부 모드' },
    { id: 'set-block', label: '차단' },
    { id: 'set-wiki', label: '위키·퀴즈' },
    { id: 'set-message', label: '알림·말투' },
    { id: 'set-link', label: '연동' },
    { id: 'set-phone', label: '폰' },
    { id: 'set-log', label: '기록' },
  ];

  return (
    <main className="page settings">
      {lock.lockHeld && (
        <p className="lock" role="note">
          <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
            <rect x="3" y="7" width="10" height="8" rx="2" fill="currentColor" />
            <path d="M5 7V5a3 3 0 0 1 6 0v2" stroke="currentColor" strokeWidth="1.6" fill="none" />
          </svg>
          {LOCK_REASON}
        </p>
      )}
      {lock.error !== null && <p className="error" role="alert">잠금 상태를 읽지 못해 설정을 바꿀 수 없습니다: {lock.error}</p>}

      <nav className="jump" aria-label="설정 묶음">
        {groups.map((g) => (
          <a key={g.id} href={`#${g.id}`}>
            {g.label}
          </a>
        ))}
      </nav>

      <section className="set" id="set-general" aria-labelledby="general-title">
        <h3 id="general-title">일반</h3>
        {readError(settingsStatus, '설정을 읽지 못했습니다')}
        {settingsStatus.kind === 'loading' && <p className="opt hint">불러오는 중</p>}
        {current && (
          <>
            <label className="opt">
              <div>
                로그인하면 자동 실행
                <small>
                  OS 등록 상태:{' '}
                  {autostartStatus.kind === 'ready'
                    ? autostartStatus.value.enabled
                      ? '등록됨'
                      : '해제됨'
                    : autostartStatus.kind === 'error'
                      ? '확인 실패'
                      : '확인 중'}
                </small>
              </div>
              <input type="checkbox" checked={current.autostart} onChange={(e) => void change('autostart', e.target.checked)} />
            </label>
            {readError(autostartStatus, 'OS 등록 상태를 읽지 못했습니다')}

            <label className="opt">
              <div>
                강제 종료되면 다시 실행
                <small>앱이 강제 종료되거나 비정상 종료되면 다시 실행해요. 자동 실행이 켜져 있어야 하고, 꺼져 있던 시간은 통계에 남아요.</small>
              </div>
              <input type="checkbox" checked={current.relaunchOnKill} onChange={(e) => void change('relaunchOnKill', e.target.checked)} />
            </label>

            <label className="opt">
              <div>
                수면에서 깨면 계획 다시 확인 표시
                <small>표시만 바뀌어요. 계획 재확인 자체는 항상 해요.</small>
              </div>
              <input
                type="checkbox"
                checked={current.recheckPlanOnWake}
                onChange={(e) => void change('recheckPlanOnWake', e.target.checked)}
              />
            </label>

            <label className="opt">
              <div>
                강제 종료 허용
                <small>실행 모드에서 정상 종료 요청과 유예 시간 뒤에도 앱이 남아 있을 때만 강제 종료해요. 저장하지 않은 작업이 사라질 수 있어요.</small>
              </div>
              <input
                type="checkbox"
                checked={current.forceQuitAllowed}
                onChange={(e) => void change('forceQuitAllowed', e.target.checked)}
              />
            </label>
          </>
        )}
      </section>

      <section className="set" id="set-mode" aria-labelledby="mode-length-title">
        <h3 id="mode-length-title">공부 모드 길이</h3>
        {readError(asyncStatus(modes.data, modes.error), '모드 길이를 읽지 못했습니다')}
        {!lock.allowed && <p className="opt hint">잠금 상태가 확인되지 않거나 진행 중이면 수정할 수 없어요.</p>}
        {(modes.data?.configs ?? []).map((config) => (
          // Remount on a saved change so the inputs show the stored values, not stale drafts.
          <div
            className="opt-block"
            key={`${config.level}-${config.studyMin}-${config.breakMin}-${config.longBreakMin}-${config.setsBeforeLongBreak}`}
          >
            <ModeConfigEditor config={config} editable={lock.allowed} onAttempted={modeAttempted} />
          </div>
        ))}
      </section>

      <section className="set" id="set-block" aria-labelledby="block-title">
        <h3 id="block-title">차단</h3>
        <BlockListPanel editable={lock.allowed} onAttempted={blockAttempted} />
        <PresetPanel editable={lock.allowed} onAttempted={blockAttempted} />
        <SiteRulesPanel editable={lock.allowed} onAttempted={blockAttempted} />
      </section>

      <section className="set" id="set-wiki" aria-labelledby="wiki-set-title">
        <h3 id="wiki-set-title">위키·퀴즈</h3>
        <QuizSettingsPanel />
        <ContextSettingsPanel />
      </section>

      <section className="set" id="set-message" aria-labelledby="message-set-title">
        <h3 id="message-set-title">알림·말투</h3>
        <MessageSettingsPanel />
      </section>

      <section className="set" id="set-link" aria-labelledby="link-set-title">
        <h3 id="link-set-title">연동</h3>
        <IntegrationsScreen
          editable={lock.allowed}
          onAttempted={() => {
            lock.refresh();
            history.reload();
          }}
        />
        <CalendarIoPanel editable={lock.allowed} onAttempted={blockAttempted} />
      </section>

      <section className="set" id="set-phone" aria-labelledby="phone-set-title">
        <h3 id="phone-set-title">폰</h3>
        <PhoneInstallPanel />
        <PhonePairPanel />
      </section>

      <section className="set" id="set-log" aria-labelledby="log-set-title">
        <h3 id="log-set-title">기록</h3>
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
              {resumed.estimated ? ' (추정)' : ''}. 계획 재확인 대상입니다.
            </p>
          )}
          {downtime.data !== null && downtime.data.length === 0 && <p className="hint">기록이 없어요.</p>}
          <ul className="list">
            {(downtime.data ?? []).slice(0, 10).map((record) => (
              <li key={record.id}>
                {record.kind === 'sleep' ? '수면' : '강제 종료'} {formatTime(record.startAtMs)} ~ {formatTime(record.endAtMs)}{' '}
                {record.estimated ? '(추정)' : ''}
              </li>
            ))}
          </ul>
          {downtime.error && <p className="error">{downtime.error}</p>}
        </section>

        <section className="card" aria-labelledby="history-title">
          <h2 id="history-title">설정 변경 기록</h2>
          <ul className="list">
            {(history.data ?? []).slice(0, 50).map((entry) => (
              <li key={entry.id}>
                <span className="mono">{formatTime(entry.atMs)}</span> · {entry.key} · {entry.before ?? '없음'} → {entry.after} ·{' '}
                {entry.reason} · {entry.outcome}
              </li>
            ))}
          </ul>
          {history.error && <p className="error">{history.error}</p>}
        </section>
      </section>

      {message && (
        <p role="status" className="message toast">
          {message}
        </p>
      )}
      {modeNotice && (
        <ModeShiftToast
          notice={modeNotice}
          onNotice={setModeNotice}
          onUndoAttempted={() => {
            lock.refresh();
            modes.reload();
            history.reload();
          }}
        />
      )}
    </main>
  );
}
