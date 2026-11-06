import { useState } from 'react';
import { commands } from '../api';
import { useAsync } from '../hooks/useAsync';
import { excerpt, noticeText, reasonLabel, sourceLabel } from '../pages/topicView';

const EXCERPT_CHARS = 600;

/** Today's topic: a read-only view of one wiki page, and a button that picks pages for the next plans. */
export function TopicPanel() {
  const topic = useAsync(commands.getTodayTopic);
  const [note, setNote] = useState<string | null>(null);

  async function assign() {
    try {
      const report = await commands.assignNextTopics();
      setNote(
        report.notice === null
          ? `${report.assigned.length}개 계획에 주제를 정했습니다.`
          : report.notice === 'no_plan'
            ? '주제를 정할 계획이 없습니다.'
            : '위키에 주제로 쓸 페이지가 없습니다.',
      );
      topic.reload();
    } catch (err) {
      setNote(`실패: ${String(err)}`);
    }
  }

  const data = topic.data;
  return (
    <section className="card" aria-labelledby="topic-title">
      <h2 id="topic-title">오늘 주제</h2>
      {topic.error !== null && <p className="error">주제를 읽지 못했습니다: {topic.error}</p>}
      {data === null && topic.error === null && <p>불러오는 중</p>}
      {data !== null && data.notice !== null && <p className="hint">{noticeText(data.notice)}</p>}
      {data !== null && data.page !== null && (
        <>
          <p>
            <strong>{data.page.pageName}</strong>
            {data.source !== null && ` — ${sourceLabel(data.source)}`}
            {data.reason !== null && ` (${reasonLabel(data.reason)})`}
          </p>
          <pre className="topic-body">{excerpt(data.page.markdown, EXCERPT_CHARS)}</pre>
        </>
      )}
      <p className="row">
        <button type="button" onClick={() => void assign()}>
          다음 주제 정하기
        </button>
      </p>
      {note !== null && <p className="hint">{note}</p>}
    </section>
  );
}
