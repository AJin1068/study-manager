import { useEffect, useState } from 'react';
import { commands, onBrowserLinkChanged, type LinkStatus, type UpdateOutcome } from '../api';
import { describeOutcome } from '../hooks/useStudyLock';
import { useAsync } from '../hooks/useAsync';
import {
  configFromDraft,
  draftFromConfig,
  linkNotice,
  type SiteRulesDraft,
} from '../pages/browserView';

interface Props {
  editable: boolean;
  onAttempted: (message: string) => void;
}

/** Browser link state, site block list, strict-mode allow list, and YouTube study entries. */
export function SiteRulesPanel({ editable, onAttempted }: Props) {
  const link = useAsync(commands.getBrowserLinkStatus);
  const enforcement = useAsync(commands.getEnforcementStatus);
  const registration = useAsync(commands.getBrowserHostRegistration);
  const rules = useAsync(commands.getSiteRules);
  const sites = useAsync(commands.listSiteEntries);
  const [draft, setDraft] = useState<SiteRulesDraft | null>(null);
  const [newSite, setNewSite] = useState('');
  const [error, setError] = useState<string | null>(null);
  const { reload: reloadLink } = link;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onBrowserLinkChanged(() => reloadLink()).then((stop: () => void) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [reloadLink]);

  async function attempt(run: () => Promise<UpdateOutcome>) {
    try {
      const outcome = await run();
      setError(null);
      onAttempted(describeOutcome(outcome));
      rules.reload();
      sites.reload();
      registration.reload();
    } catch (err) {
      setError(String(err));
    }
  }

  const current = rules.data;
  const shown = draft ?? (current ? draftFromConfig(current.config) : null);
  const status: LinkStatus | null = link.data;
  const notice = status ? linkNotice(status, enforcement.data?.active ?? false) : null;

  return (
    <section className="card" aria-labelledby="site-title">
      <h2 id="site-title">사이트 차단 (브라우저 확장)</h2>
      {link.error && <p className="error">연결 상태를 읽지 못했습니다: {link.error}</p>}
      {notice && <p className={notice.level === 'warn' ? 'warn' : undefined}>{notice.text}</p>}
      {error && <p className="error" role="alert">{error}</p>}

      {registration.data?.supported && (
        <div className="row">
          <span>{registration.data.registered ? '브라우저 연결 등록됨(Chrome)' : '브라우저 연결 등록 안 됨(Chrome)'}</span>
          <button
            type="button"
            disabled={!editable && registration.data.registered}
            onClick={() => void attempt(() => commands.setBrowserHostRegistered(!registration.data?.registered))}
          >
            {registration.data.registered ? '등록 해제' : '등록'}
          </button>
        </div>
      )}
      {registration.data && !registration.data.supported && (
        <p className="hint">이 OS에서는 브라우저 연결 등록을 지원하지 않습니다.</p>
      )}

      <h3>차단 사이트</h3>
      {sites.error && <p className="error">차단 사이트를 읽지 못했습니다: {sites.error}</p>}
      <ul className="list">
        {(sites.data ?? []).map((entry) => (
          <li key={entry.id}>
            {entry.pattern}{' '}
            {entry.source === 'preset' ? (
              <span className="hint">{entry.enabled ? '추천(켜짐)' : '추천(꺼짐)'}</span>
            ) : (
              <button type="button" disabled={!editable} onClick={() => void attempt(() => commands.removeBlockEntry(entry.id))}>
                삭제
              </button>
            )}
          </li>
        ))}
      </ul>
      <div className="row">
        <input
          type="text"
          value={newSite}
          disabled={!editable}
          placeholder="도메인 또는 도메인/경로 (예: example.com/feed)"
          onChange={(e) => setNewSite(e.target.value)}
        />
        <button
          type="button"
          disabled={!editable || newSite.trim() === ''}
          onClick={() => void attempt(() => commands.addSiteEntry(newSite)).then(() => setNewSite(''))}
        >
          추가
        </button>
      </div>

      <h3>엄격 모드와 허용 목록</h3>
      {rules.error && <p className="error">규칙을 읽지 못했습니다: {rules.error}</p>}
      {shown && current && (
        <>
          <label>
            <input
              type="checkbox"
              checked={shown.strict}
              disabled={!editable}
              onChange={(e) => setDraft({ ...shown, strict: e.target.checked })}
            />{' '}
            엄격 모드: 허용 목록의 사이트만 열기
          </label>
          <label>
            <input
              type="checkbox"
              checked={shown.customAllow}
              disabled={!editable}
              onChange={(e) => setDraft({ ...shown, customAllow: e.target.checked })}
            />{' '}
            내 허용 목록 사용
          </label>
          {shown.customAllow ? (
            <textarea
              value={shown.allowText}
              disabled={!editable}
              rows={5}
              placeholder="한 줄에 하나 (강의 사이트, 문서, 내 위키 등)"
              onChange={(e) => setDraft({ ...shown, allowText: e.target.value })}
            />
          ) : (
            <p className="hint">기본 허용: {current.strictDefaultAllow.join(', ')}</p>
          )}
          <h3>유튜브 공부 목록</h3>
          <p className="hint">등록한 재생목록과 채널 페이지는 유튜브가 차단돼도 열립니다.</p>
          <label>
            재생목록 ID (list= 값)
            <textarea
              value={shown.playlistsText}
              disabled={!editable}
              rows={3}
              onChange={(e) => setDraft({ ...shown, playlistsText: e.target.value })}
            />
          </label>
          <label>
            채널 (@핸들 또는 UC… ID)
            <textarea
              value={shown.channelsText}
              disabled={!editable}
              rows={3}
              onChange={(e) => setDraft({ ...shown, channelsText: e.target.value })}
            />
          </label>
          <button
            type="button"
            disabled={!editable || draft === null}
            onClick={() => void attempt(() => commands.updateSiteRules(configFromDraft(shown))).then(() => setDraft(null))}
          >
            저장
          </button>
        </>
      )}
    </section>
  );
}
