import type { PhoneView } from '../api';
import { blockerText, formatDuration, lockText } from '../format';

export function DiagnosticsScreen({ view, onRefresh }: { view: PhoneView | null; onRefresh: () => void }) {
  if (view === null) return <p>불러오는 중</p>;
  const report = view.report;
  return (
    <section>
      <dl>
        <dt>기기</dt>
        <dd>{view.platform}</dd>
        <dt>짝</dt>
        <dd>{view.pairingLost ? '끊김(다시 짝지어야 함)' : view.paired ? '연결됨' : '없음'}</dd>
        {report !== null && (
          <>
            <dt>잠금</dt>
            <dd>{lockText(report.diagnostics.lock)}</dd>
            <dt>임대 번호</dt>
            <dd>{report.diagnostics.seq ?? '-'}</dd>
            <dt>스스로 풀리기까지</dt>
            <dd>{report.diagnostics.lock_remaining_ms === null ? '-' : formatDuration(report.diagnostics.lock_remaining_ms)}</dd>
            <dt>마지막 서버 응답</dt>
            <dd>{report.diagnostics.last_server_contact_ago_ms === null ? '없음' : `${formatDuration(report.diagnostics.last_server_contact_ago_ms)} 전`}</dd>
            <dt>거절한 임대</dt>
            <dd>{report.diagnostics.rejected_leases}</dd>
            <dt>차단기</dt>
            <dd>{blockerText(report.blocker)}</dd>
            <dt>접근성 서비스</dt>
            <dd>{report.service_enabled ? '켜짐' : '꺼짐'}</dd>
            <dt>온라인</dt>
            <dd>{report.online ? '예' : '아니오'}</dd>
            <dt>마지막 오류</dt>
            <dd>{report.last_error ?? '없음'}</dd>
          </>
        )}
        {view.lastError !== null && (
          <>
            <dt>시작 오류</dt>
            <dd>{view.lastError}</dd>
          </>
        )}
      </dl>
      <button type="button" onClick={onRefresh}>
        지금 확인
      </button>
    </section>
  );
}
