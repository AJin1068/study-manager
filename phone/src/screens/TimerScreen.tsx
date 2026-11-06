import type { PhoneView } from '../api';
import { blockerText, formatDuration, lockIsHollow, lockText } from '../format';

export function TimerScreen({ view }: { view: PhoneView | null }) {
  if (view === null) return <p>불러오는 중</p>;
  if (!view.paired) return <p>PC와 짝지어지지 않았습니다. 짝짓기 탭에서 QR을 찍으세요.</p>;
  const report = view.report;
  if (report === null) return <p>서버에 연결하는 중{view.lastError !== null ? ` (${view.lastError})` : ''}</p>;
  const { timer, lock } = report.diagnostics;
  return (
    <section>
      <p className="timer" aria-live="off">
        {timer.state === 'studying' ? formatDuration(timer.remaining_ms) : '--:--'}
      </p>
      <p>{lockText(lock)}</p>
      <p>{blockerText(report.blocker)}</p>
      {lockIsHollow(view) && <p className="warn">잠금이 걸렸지만 차단되고 있지 않습니다. 접근성 서비스를 켜 주세요.</p>}
      {!report.online && <p>서버에 닿지 않습니다. 받아 둔 잠금은 끝날 때까지 유지됩니다.</p>}
    </section>
  );
}
