import { useCallback, useEffect, useState } from 'react';
import { commands, type PairStart, type PairStatusView } from '../api';
import { svgDataUri } from '../pages/notifyView';
import { PAIR_NOTICE, REVOKE_NOTICE, codeSecondsLeft, countdownText, pairSummary, phoneCodeText } from '../pages/phoneView';

/** Settings card: pair the phone app by QR code, see its state, end the pairing. */
export function PhonePairPanel() {
  const [status, setStatus] = useState<PairStatusView | null>(null);
  const [start, setStart] = useState<PairStart | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [note, setNote] = useState<string | null>(null);

  const load = useCallback(() => {
    commands
      .phonePairStatus()
      .then(setStatus)
      .catch((err: unknown) => setNote(`실패: ${String(err)}`));
  }, []);

  useEffect(() => {
    load();
    const timer = window.setInterval(() => {
      setNow(Date.now());
      load();
    }, 5_000);
    return () => window.clearInterval(timer);
  }, [load]);

  async function makeCode() {
    try {
      const next = await commands.phonePairStart();
      setStart(next.code === 'ok' ? next : null);
      setNote(next.code === 'ok' ? null : phoneCodeText(next.code));
    } catch (err) {
      setNote(`실패: ${String(err)}`);
    }
  }

  async function revoke() {
    try {
      const ack = await commands.phonePairRevoke();
      setNote(phoneCodeText(ack.code));
      setStart(null);
      load();
    } catch (err) {
      setNote(`실패: ${String(err)}`);
    }
  }

  const summary = status ? pairSummary(status, now) : null;
  const secondsLeft = codeSecondsLeft(start?.expiresAtMs ?? null, now);
  const showCode = start !== null && secondsLeft > 0 && !(status?.paired ?? false);

  return (
    <section className="card" aria-labelledby="phone-pair-title">
      <h2 id="phone-pair-title">폰 연결</h2>
      <p className="hint">{PAIR_NOTICE}</p>
      {summary && (
        <div data-tone={summary.tone}>
          <strong>{summary.headline}</strong>
          {summary.details.map((line) => (
            <div key={line} className="hint">{line}</div>
          ))}
        </div>
      )}
      {status?.paired ? (
        <div className="row">
          <button type="button" onClick={() => void revoke()}>연결 끊기</button>
          <span className="hint">{REVOKE_NOTICE}</span>
        </div>
      ) : (
        <div className="row">
          <button type="button" onClick={() => void makeCode()}>폰 연결 코드 만들기</button>
        </div>
      )}
      {showCode && start.qrSvg && (
        <div>
          <img src={svgDataUri(start.qrSvg)} alt="폰 연결 QR 코드" width={200} height={200} />
          <div>
            코드 <code>{start.displayCode}</code> · {countdownText(secondsLeft)}
          </div>
          <div className="hint">폰 앱에서 QR을 스캔하거나 위 코드를 입력하세요. 한 번만 쓸 수 있습니다.</div>
        </div>
      )}
      {start !== null && secondsLeft === 0 && !(status?.paired ?? false) && <p className="warn">코드가 만료되었습니다. 새 코드를 만드세요.</p>}
      {note && <p role="status" className="message">{note}</p>}
    </section>
  );
}
