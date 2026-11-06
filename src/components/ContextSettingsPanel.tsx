import { useEffect, useState } from 'react';
import { commands, type ContextSettingKey, type ContextStatus } from '../api';
import { describeOutcome } from '../hooks/useStudyLock';

const TOGGLES: ReadonlyArray<{ key: ContextSettingKey; field: 'enabled' | 'windowTitles' | 'llmConsent'; label: string }> = [
  { key: 'enabled', field: 'enabled', label: '공부 중 열어 둔 자료(앱 이름) 기록하기' },
  { key: 'windowTitles', field: 'windowTitles', label: '창 제목에서 파일 이름 읽기 (제목 원문은 저장하지 않고 80자 이내 이름만 남김)' },
  { key: 'llmConsent', field: 'llmConsent', label: '이 이름을 회고 정리용 AI 요청에 포함하기 (별도 동의)' },
];

/** Off by default. Labels stay on this device for 30 days, can be deleted at once, and reach the AI only with the last switch. */
export function ContextSettingsPanel() {
  const [status, setStatus] = useState<ContextStatus | null>(null);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    commands
      .getContextStatus()
      .then(setStatus)
      .catch((err: unknown) => setNote(`실패: ${String(err)}`));
  }, []);

  async function update(key: ContextSettingKey, value: boolean) {
    try {
      setNote(describeOutcome(await commands.updateContextSetting(key, value)));
      setStatus(await commands.getContextStatus());
    } catch (err) {
      setNote(`실패: ${String(err)}`);
    }
  }

  async function deleteAll() {
    try {
      const removed = await commands.deleteStudyContext();
      setNote(`${removed}개를 삭제했습니다.`);
      setStatus(await commands.getContextStatus());
    } catch (err) {
      setNote(`실패: ${String(err)}`);
    }
  }

  if (status === null) return note === null ? null : <p className="error">{note}</p>;

  return (
    <section className="panel" aria-label="공부 자료 기록">
      <h2>공부 자료 기록</h2>
      <p className="hint">기본값은 꺼짐입니다. 기록은 이 기기에만 {status.retentionDays}일 동안 보관됩니다.</p>
      {TOGGLES.map((toggle) => (
        <label key={toggle.key} className="row">
          <input
            type="checkbox"
            checked={status[toggle.field]}
            disabled={toggle.key !== 'enabled' && !status.enabled}
            onChange={(e) => void update(toggle.key, e.target.checked)}
          />
          {toggle.label}
        </label>
      ))}
      {status.windowTitles && status.titleAccess === 'denied' && (
        <p className="hint">창 제목을 읽으려면 macOS 손쉬운 사용 권한이 필요합니다. 없으면 앱 이름만 기록합니다.</p>
      )}
      <p className="row">
        저장된 기록 {status.rowCount}개{' '}
        <button type="button" disabled={status.rowCount === 0} onClick={() => void deleteAll()}>
          모두 삭제
        </button>
      </p>
      {note !== null && <p className="hint">{note}</p>}
    </section>
  );
}
