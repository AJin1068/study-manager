import { useEffect, useState } from 'react';
import {
  commands,
  type AskOutcome,
  type IngestView,
  type WikiPageContent,
  type WikiSourceKind,
  type WikiSubject,
} from '../api';
import { useAsync } from '../hooks/useAsync';
import { askSummary, canResume, ingestSummary, skipReasonText } from './wikiView';

/** 「위키」: subjects, material import with a preview, reading pages, and asking. */
export function WikiScreen() {
  const subjects = useAsync(commands.listWikiSubjects);
  const [subjectId, setSubjectId] = useState<number | null>(null);
  const [newName, setNewName] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [kind, setKind] = useState<WikiSourceKind>('paste');
  const [text, setText] = useState('');
  const [path, setPath] = useState('');
  const [preview, setPreview] = useState<IngestView | null>(null);
  const [picked, setPicked] = useState<string[]>([]);
  const [page, setPage] = useState<WikiPageContent | null>(null);
  const [question, setQuestion] = useState('');
  const [answer, setAnswer] = useState<AskOutcome | null>(null);
  const [pages, setPages] = useState<{ pageName: string }[]>([]);

  const list = subjects.data ?? [];
  const subject: WikiSubject | undefined = list.find((s) => s.id === subjectId) ?? list[0];
  const activeId = subject?.id ?? null;

  useEffect(() => {
    if (activeId === null) return;
    commands
      .listWikiPages(activeId)
      .then(setPages)
      .catch((err: unknown) => setError(String(err)));
  }, [activeId, subjects.data]);

  async function run<T>(job: () => Promise<T>, done?: (value: T) => void) {
    setBusy(true);
    setError(null);
    try {
      const value = await job();
      done?.(value);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
      subjects.reload();
    }
  }

  function showPreview(view: IngestView) {
    setPreview(view);
    setPicked(view.proposals.map((p) => p.pageName));
    setMessage(ingestSummary(view));
  }

  async function apply() {
    if (!preview?.previewId) return;
    const id = preview.previewId;
    await run(
      () => commands.applyWikiIngest(id, picked),
      (report) => {
        setPreview(null);
        setMessage(`${report.written.length}개 페이지를 저장했습니다.${report.skipped.length > 0 ? ` (${report.skipped.length}개 건너뜀)` : ''}`);
      },
    );
  }

  async function discard() {
    if (preview?.previewId) await commands.cancelWikiIngest(preview.previewId).catch(() => undefined);
    setPreview(null);
  }

  return (
    <section className="card" aria-labelledby="wiki-title">
      <h2 id="wiki-title">위키</h2>
      <p className="hint">
        과목마다 마크다운 폴더가 하나씩 있습니다. 자료는 먼저 초안으로 보여 드리고, 확인한 것만 파일에 저장합니다.
        LLM이 꺼져 있어도 제목 단위로 나누는 기본 정리는 됩니다.
      </p>
      {error && <p className="error" role="alert">{error}</p>}
      {subjects.error && <p className="error">과목을 읽지 못했습니다: {subjects.error}</p>}
      {message && <p role="status" className="message">{message}</p>}

      <div className="row">
        <label>
          과목{' '}
          <select value={activeId ?? ''} onChange={(e) => { setSubjectId(Number(e.target.value)); setPreview(null); setAnswer(null); setPage(null); }}>
            {list.map((s) => (
              <option key={s.id} value={s.id}>{s.name}</option>
            ))}
          </select>
        </label>
        <input value={newName} placeholder="새 과목 이름" onChange={(e) => setNewName(e.target.value)} />
        <button
          type="button"
          disabled={busy || newName.trim() === ''}
          onClick={() => void run(() => commands.createWikiSubject(newName), (created) => { setSubjectId(created.id); setNewName(''); })}
        >
          과목 추가
        </button>
        {subject && (
          <button type="button" disabled={busy} onClick={() => void run(() => commands.setWikiSubjectArchived(subject.id, true), () => setSubjectId(null))}>
            보관
          </button>
        )}
      </div>
      {list.length === 0 && <p>과목이 없습니다. 계획에 과목을 넣거나 위에서 추가하세요.</p>}
      {subject?.folderMissing && (
        <p className="warn">이 과목의 폴더가 없습니다. 페이지를 저장하면 다시 만들어집니다.</p>
      )}

      {subject && (
        <>
          <h3>자료 넣기</h3>
          <div className="row" role="group" aria-label="자료 종류">
            {([['paste', '붙여넣기'], ['note', '메모'], ['link', '링크 + 메모']] as const).map(([value, label]) => (
              <button key={value} type="button" aria-pressed={kind === value} onClick={() => setKind(value)}>{label}</button>
            ))}
          </div>
          {kind === 'link' && <p className="hint">링크는 열지 않습니다. 링크와 함께 그 내용을 요약한 메모를 적어 주세요.</p>}
          <textarea rows={5} value={text} aria-label="자료 내용" onChange={(e) => setText(e.target.value)} />
          <div className="row">
            <button
              type="button"
              disabled={busy || text.trim() === ''}
              onClick={() => void run(() => commands.ingestWikiText(subject.id, kind, '', text), (view) => { showPreview(view); setText(''); })}
            >
              정리하기
            </button>
            <button type="button" disabled={busy} onClick={() => void run(() => commands.draftWikiOutline(subject.id), showPreview)}>
              과목 이름으로 목차 초안
            </button>
          </div>
          <div className="row">
            <input value={path} placeholder="파일 또는 폴더의 절대 경로 (.md .txt .pdf)" aria-label="파일 또는 폴더 경로" onChange={(e) => setPath(e.target.value)} />
            <button type="button" disabled={busy || path.trim() === ''} onClick={() => void run(() => commands.ingestWikiFile(subject.id, path.trim()), showPreview)}>
              파일 가져오기
            </button>
            <button type="button" disabled={busy || path.trim() === ''} onClick={() => void run(() => commands.ingestWikiFolder(subject.id, path.trim()), showPreview)}>
              폴더 가져오기
            </button>
          </div>

          {preview && (
            <div className="card">
              <h3>저장 전 확인</h3>
              <p>{ingestSummary(preview)}</p>
              {preview.skipped.length > 0 && (
                <ul className="list">
                  {preview.skipped.map((f) => (
                    <li key={`${f.name}:${f.reason}`}>{f.name} — {skipReasonText(f.reason)}</li>
                  ))}
                </ul>
              )}
              {preview.proposals.map((p) => (
                <details key={p.pageName}>
                  <summary>
                    <label>
                      <input
                        type="checkbox"
                        checked={picked.includes(p.pageName)}
                        onChange={(e) => setPicked(e.target.checked ? [...picked, p.pageName] : picked.filter((n) => n !== p.pageName))}
                      />{' '}
                      {p.pageName} ({p.mode === 'append' ? '기존 페이지에 이어 붙임, 원본은 백업' : '새 페이지'})
                    </label>
                  </summary>
                  <pre>{p.markdown}</pre>
                </details>
              ))}
              <div className="row">
                <button type="button" disabled={busy || picked.length === 0 || preview.previewId === null} onClick={() => void apply()}>
                  선택한 {picked.length}개 저장
                </button>
                {canResume(preview) && preview.previewId && (
                  <button type="button" disabled={busy} onClick={() => void run(() => commands.resumeWikiIngest(preview.previewId ?? ''), showPreview)}>
                    이어서 처리
                  </button>
                )}
                <button type="button" disabled={busy} onClick={() => void discard()}>버리기</button>
              </div>
            </div>
          )}

          <h3>페이지</h3>
          <div className="row">
            <button type="button" disabled={busy} onClick={() => void run(() => commands.reindexWiki(subject.id), (r) => setMessage(`색인을 다시 만들었습니다: ${r.pages}개 (추가 ${r.added}, 갱신 ${r.updated}, 제거 ${r.removed})`))}>
              색인 다시 만들기
            </button>
          </div>
          {pages.length === 0 && <p>아직 페이지가 없습니다.</p>}
          <ul className="list">
            {pages.map((p) => (
              <li key={p.pageName}>
                <button type="button" onClick={() => void run(() => commands.readWikiPage(subject.id, p.pageName), setPage)}>{p.pageName}</button>
              </li>
            ))}
          </ul>
          {page && <pre>{page.markdown}</pre>}

          <h3>묻기</h3>
          <div className="row">
            <input value={question} placeholder="이 과목 위키에 물어보기" aria-label="질문" onChange={(e) => setQuestion(e.target.value)} />
            <button type="button" disabled={busy || question.trim() === ''} onClick={() => void run(() => commands.askWiki(subject.id, question), setAnswer)}>
              묻기
            </button>
          </div>
          {answer && (
            <div>
              {askSummary(answer) !== '' && <p className="hint">{askSummary(answer)}</p>}
              {answer.status === 'answer' && (
                <>
                  <p>{answer.answer}</p>
                  <p className="hint">근거: {answer.citations.join(', ')}</p>
                  <button type="button" disabled={busy} onClick={() => void run(() => commands.previewWikiAnswerPage(answer.askId), showPreview)}>
                    답을 페이지로 저장(미리보기)
                  </button>
                </>
              )}
              {(answer.status === 'ungrounded' || answer.status === 'unavailable') && (
                <ul className="list">
                  {answer.matches.map((m) => (
                    <li key={m.pageName}><strong>{m.pageName}</strong> {m.excerpt}</li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </>
      )}
    </section>
  );
}
