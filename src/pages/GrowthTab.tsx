import { useEffect, useState } from 'react';
import { commands, type WeeklyInsight } from '../api';
import { useAsync } from '../hooks/useAsync';
import { describeOutcome } from '../hooks/useStudyLock';
import { accuracyText, insightNote, levelText, reviewText, totalTimeText, wikiText } from './growthView';

function InsightCard() {
  const [insight, setInsight] = useState<WeeklyInsight | null>(null);
  const [withSubject, setWithSubject] = useState<boolean | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  function refresh() {
    setBusy(true);
    commands
      .getWeeklyInsight(true)
      .then((value) => {
        setInsight(value);
        setNote(null);
      })
      .catch((err: unknown) => setNote(`읽지 못했습니다: ${String(err)}`))
      .finally(() => setBusy(false));
  }

  useEffect(() => {
    commands
      .getWeeklyInsight(false)
      .then(setInsight)
      .catch((err: unknown) => setNote(`읽지 못했습니다: ${String(err)}`));
    commands
      .getInsightSubject()
      .then(setWithSubject)
      .catch((err: unknown) => setNote(`읽지 못했습니다: ${String(err)}`));
  }, []);

  async function toggleSubject(value: boolean) {
    try {
      const outcome = await commands.setInsightSubject(value);
      setNote(describeOutcome(outcome));
      setWithSubject(await commands.getInsightSubject());
    } catch (err) {
      setNote(`실패: ${String(err)}`);
    }
  }

  return (
    <section className="card" aria-labelledby="insight-title">
      <h2 id="insight-title">이번 주 한 줄</h2>
      {insight === null ? <p>불러오는 중</p> : <p><strong>{insight.text}</strong></p>}
      {insight !== null && insightNote(insight) !== null && <p className="hint">{insightNote(insight)}</p>}
      <div className="row">
        <button type="button" disabled={busy} onClick={refresh}>다시 만들기</button>
        {withSubject !== null && (
          <label>
            <input type="checkbox" checked={withSubject} onChange={(e) => void toggleSubject(e.target.checked)} /> 모델에 가장 약한 과목 이름 함께 보내기
          </label>
        )}
      </div>
      <p className="hint">모델에는 이번 주 집계 숫자만 보냅니다. 창 제목, 주소, 앱 이름은 보내지 않습니다.</p>
      {note !== null && <p role="status" className="message">{note}</p>}
    </section>
  );
}

/** Growth numbers and the weekly insight. Everything except the insight sentence is local. */
export function GrowthTab() {
  const growth = useAsync(commands.getGrowthStatus);
  const status = growth.data;
  return (
    <>
      {growth.error !== null && <p className="error">읽지 못했습니다: {growth.error}</p>}
      {status === null && growth.error === null && <p>불러오는 중</p>}
      {status !== null && (
        <section className="card" aria-labelledby="growth-title">
          <h2 id="growth-title">성장</h2>
          <p>{totalTimeText(status)}</p>
          <p>{levelText(status)}</p>
          <div className="bar" role="img" aria-label={`레벨 진행 ${status.levelProgressPct}%`}>
            <div className="bar-fill" style={{ width: `${status.levelProgressPct}%` }} />
          </div>
          <p>{wikiText(status)}</p>
          <p>{accuracyText(status)}</p>
          <p>
            다음 복습: <strong>{reviewText(status.nextReview)}</strong>
          </p>
        </section>
      )}
      <InsightCard />
    </>
  );
}
