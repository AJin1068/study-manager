import { useState } from 'react';
import { commands, type IcsApplyResult, type IcsPreview } from '../api';
import { describeOutcome } from '../hooks/useStudyLock';
import { useAsync } from '../hooks/useAsync';
import {
  describeApplied,
  describePreview,
  describePreviewItem,
  EXPORT_FILE_NAME,
  undoableBatch,
} from '../pages/calendarIoLogic';
import { planChangeMessage } from '../pages/calendarLogic';

interface Props {
  /** Controls may be edited: lock state known and free. The Rust core enforces it again. */
  editable: boolean;
  /** Called after any attempt so the parent can re-read the lock and history. */
  onAttempted: (message: string) => void;
}

function saveTextFile(name: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: 'text/calendar;charset=utf-8' }));
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}

/** Calendar file (.ics) import and export. Off by default; files stay on this computer. */
export function CalendarIoPanel({ editable, onAttempted }: Props) {
  const enabled = useAsync(commands.getCalendarIoEnabled);
  const [content, setContent] = useState<string | null>(null);
  const [preview, setPreview] = useState<IcsPreview | null>(null);
  const [applied, setApplied] = useState<IcsApplyResult | null>(null);
  const [exported, setExported] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  async function run(action: () => Promise<void>) {
    try {
      await action();
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  }

  const toggle = (on: boolean) =>
    run(async () => {
      const outcome = await commands.updateSetting('calendarIoEnabled', on);
      enabled.reload();
      setPreview(null);
      setContent(null);
      setNotice(describeOutcome(outcome));
      onAttempted(describeOutcome(outcome));
    });

  const exportFile = () =>
    run(async () => {
      const result = await commands.exportPlansIcs();
      saveTextFile(EXPORT_FILE_NAME, result.content);
      setExported(result.content);
      setNotice(`계획 ${result.eventCount}개를 ${EXPORT_FILE_NAME}로 내보냈습니다.`);
    });

  const pick = (file: File | undefined) =>
    run(async () => {
      setApplied(null);
      setPreview(null);
      if (!file) return;
      const text = await file.text();
      setContent(text);
      setPreview(await commands.previewIcsImport(text));
    });

  const apply = () =>
    run(async () => {
      if (content === null) return;
      const result = await commands.applyIcsImport(content);
      setApplied(result);
      setPreview(null);
      setContent(null);
      setNotice(describeApplied(result));
      onAttempted(describeApplied(result));
    });

  const undo = (batchId: string) =>
    run(async () => {
      const change = await commands.undoIcsImport(batchId);
      const message = planChangeMessage(change);
      setApplied(null);
      setNotice(message.text);
      onAttempted(message.text);
    });

  const on = enabled.data === true;
  const batch = undoableBatch(applied);

  return (
    <section className="card" aria-labelledby="calendar-io-title">
      <h2 id="calendar-io-title">캘린더 가져오기·내보내기</h2>
      {enabled.error && <p className="error">설정을 읽지 못했습니다: {enabled.error}</p>}
      {error && <p className="error" role="alert">{error}</p>}
      <label className="row">
        <input
          type="checkbox"
          checked={on}
          disabled={!editable || enabled.data === null}
          onChange={(e) => void toggle(e.target.checked)}
        />
        .ics 파일로 가져오기·내보내기 사용
      </label>
      <p className="hint">
        기본은 꺼짐입니다. 파일은 이 컴퓨터에서만 읽고 저장하며 어디로도 전송하지 않습니다. 구글·애플 계정 연동은 아닙니다.
      </p>
      {on && (
        <>
          <div className="row">
            <button type="button" onClick={() => void exportFile()}>계획 내보내기 (.ics)</button>
          </div>
          {exported !== null && (
            <textarea readOnly rows={4} aria-label="내보낸 .ics 내용" value={exported} />
          )}
          <label className="row">
            가져오기:
            <input
              type="file"
              accept=".ics,text/calendar"
              disabled={!editable}
              onChange={(e) => void pick(e.target.files?.[0])}
            />
          </label>
          {preview && (
            <>
              <p role="status">{describePreview(preview)}</p>
              <ul className="list">
                {preview.items.slice(0, 50).map((item, i) => (
                  <li key={`${item.startDate}-${item.startTime}-${i}`}>{describePreviewItem(item)}</li>
                ))}
              </ul>
              {preview.items.length > 50 && <p className="hint">… 외 {preview.items.length - 50}개</p>}
              <div className="row">
                <button type="button" disabled={!editable || preview.items.length === 0} onClick={() => void apply()}>
                  계획 {preview.items.length}개 가져오기
                </button>
              </div>
            </>
          )}
          {batch !== null && (
            <div className="row">
              <button type="button" disabled={!editable} onClick={() => void undo(batch)}>
                가져오기 되돌리기
              </button>
            </div>
          )}
        </>
      )}
      {notice && <p role="status">{notice}</p>}
    </section>
  );
}
