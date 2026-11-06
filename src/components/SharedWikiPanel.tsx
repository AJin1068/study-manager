import { useCallback, useEffect, useState } from 'react';
import { commands, type SharedQuizQuestion, type SharedRoomView } from '../api';
import { canMerge, mergeSummary, pageSummary } from '../pages/sharedWikiView';

/** The open room's shared wiki (mount with `key` = room): pages, new concepts, merge, mutual quiz and the daily recap. */
export function SharedWikiPanel(props: { roomId: string }) {
  const { roomId } = props;
  const [view, setView] = useState<SharedRoomView | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [round, setRound] = useState<SharedQuizQuestion[]>([]);
  const [verdicts, setVerdicts] = useState<Record<number, boolean | null>>({});

  const load = useCallback(() => {
    commands
      .sharedRoom(roomId)
      .then((v) => {
        setView(v);
        setMessage(null);
      })
      .catch((err: unknown) => setMessage(String(err)));
  }, [roomId]);

  useEffect(() => {
    load();
  }, [load]);

  function answer(itemId: number, text: string) {
    commands
      .answerSharedQuiz(itemId, text)
      .then((ok) => setVerdicts((v) => ({ ...v, [itemId]: ok })))
      .catch((err: unknown) => setMessage(String(err)));
  }

  if (view === null) return message === null ? null : <p role="status">{message}</p>;

  return (
    <section className="card">
      <h2>공유 위키</h2>
      <p className="hint">{view.shareNotice}</p>
      <p>{view.summary}</p>

      {view.newConcepts.length > 0 && (
        <div role="status">
          <h3>내 위키에 없는 개념</h3>
          <ul className="list">
            {view.newConcepts.map((c) => (
              <li key={c.title}>
                {c.title} <span className="hint">({c.authors.join(', ')}이(가) 정리)</span>
              </li>
            ))}
          </ul>
          <button
            type="button"
            onClick={() => {
              commands
                .acknowledgeConcepts(roomId, view.newConcepts.map((c) => c.title))
                .then(load)
                .catch((err: unknown) => setMessage(String(err)));
            }}
          >
            확인했어요
          </button>
        </div>
      )}

      <h3>페이지</h3>
      {view.pages.length === 0 && <p>아직 공유된 페이지가 없어요.</p>}
      <ul className="list">
        {view.pages.map((page) => (
          <li key={page.title}>
            <strong>{page.title}</strong> <span className="hint">{pageSummary(page)}</span>{' '}
            <button
              type="button"
              disabled={!canMerge(page)}
              onClick={() => {
                commands
                  .mergeShared(roomId, page.title)
                  .then((m) => {
                    load();
                    setMessage(mergeSummary(m));
                  })
                  .catch((err: unknown) => setMessage(String(err)));
              }}
            >
              합치기
            </button>
          </li>
        ))}
      </ul>

      <h3>서로 퀴즈</h3>
      <button
        type="button"
        onClick={() => {
          setVerdicts({});
          commands.startSharedQuiz(roomId).then(setRound).catch((err: unknown) => setMessage(String(err)));
        }}
      >
        공유 페이지로 퀴즈 풀기
      </button>
      <ol>
        {round.map((q) => (
          <li key={q.itemId}>
            <p>
              {q.prompt} <span className="hint">({q.pageName})</span>
            </p>
            {q.kind === 'mcq' ? (
              <div className="row">
                {q.choices.map((choice) => (
                  <button key={choice} type="button" onClick={() => answer(q.itemId, choice)}>
                    {choice}
                  </button>
                ))}
              </div>
            ) : (
              <ClozeAnswer onSubmit={(text) => answer(q.itemId, text)} />
            )}
            {q.itemId in verdicts && <p role="status">{verdicts[q.itemId] === true ? '정답!' : '아쉬워요.'}</p>}
          </li>
        ))}
      </ol>
      {message !== null && <p role="status">{message}</p>}
    </section>
  );
}

function ClozeAnswer(props: { onSubmit: (text: string) => void }) {
  const [text, setText] = useState('');
  return (
    <div className="row">
      <input value={text} aria-label="답" onChange={(e) => setText(e.target.value)} />
      <button type="button" disabled={text.trim() === ''} onClick={() => props.onSubmit(text)}>
        제출
      </button>
    </div>
  );
}
