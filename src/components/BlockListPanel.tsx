import { useEffect, useState } from 'react';
import { commands, onBlockAttempt, type RunningApp, type UpdateOutcome } from '../api';
import { describeOutcome } from '../hooks/useStudyLock';
import { useAsync } from '../hooks/useAsync';
import { asyncStatus } from '../hooks/useAsyncStatus';
import { actionLabel, detailLabel, parseGrace } from '../pages/enforceView';

interface Props {
  /** Controls may be edited: lock state known and free. The Rust core enforces it again. */
  editable: boolean;
  /** Called after any attempt so the parent can re-read the lock and history. */
  onAttempted: (message: string) => void;
}

export function BlockListPanel({ editable, onAttempted }: Props) {
  const status = useAsync(commands.getEnforcementStatus);
  const entries = useAsync(commands.listBlockEntries);
  const [running, setRunning] = useState<RunningApp[] | null>(null);
  const [manual, setManual] = useState('');
  const [confirmation, setConfirmation] = useState('');
  const [grace, setGrace] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { reload: reloadStatus } = status;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onBlockAttempt(() => reloadStatus()).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [reloadStatus]);

  async function attempt(run: () => Promise<UpdateOutcome>) {
    setError(null);
    try {
      onAttempted(describeOutcome(await run()));
    } catch (err) {
      setError(String(err));
    }
    status.reload();
    entries.reload();
  }

  async function loadRunning() {
    setError(null);
    try {
      setRunning(await commands.listRunningApps());
    } catch (err) {
      setRunning(null);
      setError(`실행 중인 앱을 읽지 못했습니다: ${String(err)}`);
    }
  }

  const view = asyncStatus(status.data, status.error);
  const current = view.kind === 'ready' ? view.value : null;
  const graceText = grace ?? String(current?.graceWaitSec ?? '');

  return (
    <section className="card" aria-labelledby="block-title">
      <h2 id="block-title">앱 차단</h2>
      {view.kind === 'error' && <p className="error">차단 상태를 읽지 못했습니다: {view.message}</p>}
      {error && <p className="error" role="alert">{error}</p>}
      {current && (
        <>
          <p>
            모드: <strong>{current.mode === 'dryRun' ? '시험 모드(기록만 함)' : '실행 모드'}</strong>
            {current.active ? ' · 지금 차단 중' : ''}
          </p>
          {current.mode === 'dryRun' ? (
            <div className="row">
              <input
                type="text"
                value={confirmation}
                disabled={!editable}
                placeholder={`"${current.enforceConfirmation}" 입력`}
                onChange={(e) => setConfirmation(e.target.value)}
              />
              <button
                type="button"
                disabled={!editable}
                onClick={() => void attempt(() => commands.setEnforcementMode('enforce', confirmation))}
              >
                실행 모드 켜기
              </button>
            </div>
          ) : (
            <button
              type="button"
              disabled={!editable}
              onClick={() => void attempt(() => commands.setEnforcementMode('dryRun', ''))}
            >
              시험 모드로 되돌리기
            </button>
          )}
          <p className="hint">
            실행 모드에서는 차단 목록의 앱에 정상 종료를 먼저 요청합니다. 강제 종료는 위의 &quot;강제 종료 허용&quot;을
            켠 경우에만, 유예 시간이 지난 뒤 수행합니다.
          </p>
          <div className="row">
            <label>
              정상 종료 대기(초){' '}
              <input
                type="number"
                min={1}
                max={60}
                value={graceText}
                disabled={!editable}
                onChange={(e) => setGrace(e.target.value)}
              />
            </label>
            <button
              type="button"
              disabled={!editable || grace === null}
              onClick={() => {
                const parsed = parseGrace(graceText);
                if (!parsed.ok) {
                  setError(parsed.error);
                  return;
                }
                void attempt(() => commands.updateSetting('graceWaitSec', parsed.value)).then(() => setGrace(null));
              }}
            >
              저장
            </button>
          </div>
        </>
      )}

      <h3>차단 목록</h3>
      {entries.error && <p className="error">차단 목록을 읽지 못했습니다: {entries.error}</p>}
      <ul className="list">
        {(entries.data ?? []).map((entry) => (
          <li key={entry.id}>
            {entry.pattern}{' '}
            {entry.source === 'preset' ? (
              <span className="hint">{entry.enabled ? '추천(켜짐)' : '추천(꺼짐)'} · 카테고리에서 켜고 끕니다</span>
            ) : (
              <button type="button" disabled={!editable} onClick={() => void attempt(() => commands.removeBlockEntry(entry.id))}>
                삭제
              </button>
            )}
          </li>
        ))}
      </ul>
      {entries.data?.length === 0 && <p className="hint">차단할 앱이 없습니다.</p>}
      <div className="row">
        <input
          type="text"
          value={manual}
          disabled={!editable}
          placeholder="앱 ID (macOS 번들 ID, Windows 실행 파일 경로)"
          onChange={(e) => setManual(e.target.value)}
        />
        <button
          type="button"
          disabled={!editable || manual.trim() === ''}
          onClick={() => void attempt(() => commands.addBlockEntry(manual)).then(() => setManual(''))}
        >
          추가
        </button>
        <button type="button" onClick={() => void loadRunning()}>실행 중인 앱에서 고르기</button>
      </div>
      {running && (
        <ul className="list">
          {running.map((app) => (
            <li key={app.id}>
              {app.displayName} ({app.id}){' '}
              {app.isProtected ? (
                <span className="hint">보호됨</span>
              ) : (
                <button type="button" disabled={!editable} onClick={() => void attempt(() => commands.addBlockEntry(app.id))}>
                  차단 목록에 추가
                </button>
              )}
            </li>
          ))}
        </ul>
      )}

      <h3>최근 차단 기록</h3>
      <ul className="list">
        {(current?.recentEvents ?? []).map((event) => (
          <li key={event.id}>
            {new Date(event.atMs).toLocaleString('ko-KR')} · {event.targetId} · {actionLabel(event.action)}
            {event.detailCode ? ` (${detailLabel(event.detailCode)})` : ''}
          </li>
        ))}
      </ul>
    </section>
  );
}
