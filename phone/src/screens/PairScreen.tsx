import { useState } from 'react';
import { phoneApi, scanQr, type PhoneView } from '../api';
import { errorText } from '../format';

export function PairScreen({ view, onChanged }: { view: PhoneView | null; onChanged: () => void }) {
  const [text, setText] = useState('');
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const run = async (action: () => Promise<{ code: string }>, done: string) => {
    setBusy(true);
    try {
      const ack = await action();
      setMessage(ack.code === 'ok' ? done : errorText(ack.code));
      if (ack.code === 'ok') onChanged();
    } catch (err) {
      setMessage(`실패: ${String(err)}`);
    } finally {
      setBusy(false);
    }
  };

  const scan = async () => {
    try {
      const content = await scanQr();
      setText(content);
      await run(() => phoneApi.pair(content), '짝지었습니다.');
    } catch (err) {
      setMessage(`카메라를 쓸 수 없습니다: ${String(err)}`);
    }
  };

  return (
    <section>
      {view?.pairingLost === true && <p className="warn">저장된 로그인이 거절되었습니다. PC에서 새 코드를 만들어 다시 짝지으세요.</p>}
      <p>PC 앱의 폰 연결 화면에 뜬 QR을 찍으세요. 코드는 5분 동안, 한 번만 쓸 수 있습니다.</p>
      <button type="button" disabled={busy} onClick={() => void scan()}>
        QR 찍기
      </button>
      <p>카메라를 쓸 수 없으면 QR의 글자를 붙여 넣으세요.</p>
      <textarea value={text} onChange={(event) => setText(event.target.value)} aria-label="QR 글자" />
      <button type="button" disabled={busy || text.trim() === ''} onClick={() => void run(() => phoneApi.pair(text), '짝지었습니다.')}>
        짝짓기
      </button>
      {view?.paired === true && (
        <button type="button" disabled={busy} onClick={() => void run(phoneApi.unpair, '짝을 끊었습니다.')}>
          짝 끊기
        </button>
      )}
      {message !== null && <p role="status">{message}</p>}
    </section>
  );
}
