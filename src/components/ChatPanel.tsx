import { useRef, useState, type FormEvent } from 'react';
import { commands, type ChatDraft, type ChatMessage } from '../api';
import { KIND_LABEL, applyNotice, canSend, replyNotice, toHistory } from '../pages/chatView';

interface Line extends ChatMessage {
  draft?: ChatDraft;
  notice?: string | null;
}

/**
 * Planning chat. The conversation is kept here only (it is not stored anywhere); a draft is a
 * faded preview that changes nothing until "적용". The core applies it through the study lock.
 */
export function ChatPanel(props: { compact?: boolean }) {
  const [lines, setLines] = useState<Line[]>([]);
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const history = useRef<Line[]>([]);

  function push(next: Line[]) {
    history.current = next;
    setLines(next);
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    const message = text.trim();
    if (!canSend(message, busy)) return;
    const before = history.current;
    push([...before, { role: 'user', text: message }]);
    setText('');
    setBusy(true);
    setStatus(null);
    try {
      const reply = await commands.sendChatMessage(toHistory(before), message);
      push([...history.current, { role: 'assistant', text: reply.text, draft: reply.draft ?? undefined, notice: replyNotice(reply) }]);
    } catch (err) {
      setStatus(`실패: ${String(err)}`);
    } finally {
      setBusy(false);
    }
  }

  function clearDraft(draftId: number) {
    push(history.current.map((l) => (l.draft?.id === draftId ? { ...l, draft: undefined } : l)));
  }

  async function apply(draftId: number) {
    try {
      const result = await commands.applyChatDraft(draftId);
      setStatus(applyNotice(result));
      if (result.applied) clearDraft(draftId);
    } catch (err) {
      setStatus(`실패: ${String(err)}`);
    }
  }

  async function discard(draftId: number) {
    try {
      await commands.discardChatDraft(draftId);
      clearDraft(draftId);
      setStatus('초안을 버렸습니다.');
    } catch (err) {
      setStatus(`실패: ${String(err)}`);
    }
  }

  return (
    <section className={props.compact ? 'chat compact' : 'card chat'} aria-label="계획 챗봇">
      {!props.compact && <h2>계획 챗봇</h2>}
      <div className="chat-log" role="log" aria-live="polite">
        {lines.length === 0 && <p className="hint">예: 「다음 주 토요일 시험인데 평일 저녁만 쓸 수 있어」 · 「이번 주 빈 시간 알려줘」</p>}
        {lines.map((line, index) => (
          <div key={index} className={`chat-line ${line.role}`}>
            <p>{line.text}</p>
            {line.notice && <p className="hint">{line.notice}</p>}
            {line.draft && (
              <div className="chat-draft" aria-label="계획 초안 미리보기">
                <ul className="list">
                  {line.draft.items.map((item, i) => (
                    <li key={i}>
                      <strong>{KIND_LABEL[item.kind]}</strong> {item.label}
                    </li>
                  ))}
                </ul>
                <p className="hint">아직 계획이 바뀌지 않았습니다.</p>
                <div className="row">
                  <button type="button" onClick={() => void apply(line.draft?.id ?? 0)}>적용</button>
                  <button type="button" onClick={() => void discard(line.draft?.id ?? 0)}>버리기</button>
                </div>
              </div>
            )}
          </div>
        ))}
      </div>
      <form className="row" onSubmit={(e) => void submit(e)}>
        <input type="text" value={text} onChange={(e) => setText(e.target.value)} maxLength={1000} aria-label="챗봇에게 보낼 말" disabled={busy} />
        <button type="submit" disabled={!canSend(text, busy)}>{busy ? '생각 중' : '보내기'}</button>
      </form>
      {status !== null && <p role="status" className="message">{status}</p>}
      {!props.compact && (
        <div className="row">
          <button type="button" onClick={() => void commands.openChatInput().catch((err: unknown) => setStatus(`실패: ${String(err)}`))}>
            작은 입력창 열기
          </button>
        </div>
      )}
    </section>
  );
}
