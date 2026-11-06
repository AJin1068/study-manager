import { useEffect, useState } from 'react';
import { commands, onRemoteState, type LlmSettingKey } from '../api';
import { describeOutcome } from '../hooks/useStudyLock';
import { useAsync } from '../hooks/useAsync';
import { budgetSummary, purposeLabel } from './llmView';

interface Props {
  editable: boolean;
  onAttempted: () => void;
}

/** LLM switches and today's usage. The API key itself is entered under 「키」. */
export function LlmCard({ editable, onAttempted }: Props) {
  const config = useAsync(commands.getLlmSettings);
  const status = useAsync(commands.getLlmStatus);
  const [model, setModel] = useState<string | null>(null);
  const [budget, setBudget] = useState<string | null>(null);
  const [wikiDir, setWikiDir] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const reloadStatus = status.reload;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onRemoteState(reloadStatus).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [reloadStatus]);

  async function save(key: LlmSettingKey, value: string | number | boolean) {
    try {
      setMessage(describeOutcome(await commands.updateLlmSetting(key, value)));
      setError(null);
    } catch (err) {
      setError(String(err));
    } finally {
      config.reload();
      status.reload();
      onAttempted();
    }
  }

  const stored = config.data;
  const current = status.data;
  return (
    <>
      <h3>LLM (위키 정리·묻기)</h3>
      <p className="hint">
        기본은 꺼짐입니다. 켜면 위키 정리·묻기를 할 때 해당 자료 조각과 질문만 Anthropic으로 보냅니다.
        창 제목·주소·입력한 문장은 보내지 않습니다. 꺼져 있어도 위키는 직접 정리할 수 있습니다.
      </p>
      {error && <p className="error" role="alert">{error}</p>}
      {config.error && <p className="error">LLM 설정을 읽지 못했습니다: {config.error}</p>}
      <div className="row">
        <label>
          <input
            type="checkbox"
            checked={stored?.enabled ?? false}
            disabled={!editable || stored === null}
            onChange={(e) => void save('enabled', e.target.checked)}
          />{' '}
          LLM 사용
        </label>
      </div>
      <div className="row">
        <label>
          모델{' '}
          <input value={model ?? stored?.model ?? ''} disabled={!editable} onChange={(e) => setModel(e.target.value)} />
        </label>
        <button type="button" disabled={!editable || model === null} onClick={() => void save('model', model ?? '').then(() => setModel(null))}>
          저장
        </button>
      </div>
      <div className="row">
        <label>
          하루 토큰 한도{' '}
          <input inputMode="numeric" value={budget ?? String(stored?.dailyBudgetTokens ?? '')} disabled={!editable} onChange={(e) => setBudget(e.target.value)} />
        </label>
        <button
          type="button"
          disabled={!editable || budget === null || !/^\d+$/.test(budget)}
          onClick={() => void save('dailyBudgetTokens', Number(budget)).then(() => setBudget(null))}
        >
          저장
        </button>
      </div>
      <div className="row">
        <label>
          위키 폴더(절대 경로, 비우면 기본 위치){' '}
          <input value={wikiDir ?? stored?.wikiDir ?? ''} disabled={!editable} onChange={(e) => setWikiDir(e.target.value)} />
        </label>
        <button type="button" disabled={!editable || wikiDir === null} onClick={() => void save('wikiDir', wikiDir ?? '').then(() => setWikiDir(null))}>
          저장
        </button>
      </div>
      {current && (
        <>
          <p>{budgetSummary(current)}</p>
          <ul className="list">
            {current.byPurpose.map((row) => (
              <li key={row.purpose}>
                {purposeLabel(row.purpose)} · {row.calls}회 · {row.tokens.toLocaleString('ko-KR')} 토큰
              </li>
            ))}
          </ul>
        </>
      )}
      {message && <p role="status" className="message">{message}</p>}
    </>
  );
}
