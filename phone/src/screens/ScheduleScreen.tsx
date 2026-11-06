import { useEffect, useState } from 'react';
import { phoneApi, type ScheduleView } from '../api';

export function ScheduleScreen() {
  const [schedule, setSchedule] = useState<ScheduleView | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    phoneApi.schedule().then(setSchedule).catch((err: unknown) => setError(String(err)));
  }, []);

  if (error !== null) return <p className="warn">시간표를 읽지 못했습니다: {error}</p>;
  if (schedule === null) return <p>불러오는 중</p>;
  if (schedule.items.items.length === 0) {
    return <p>표시할 시간표가 없습니다. 서버에서 폰으로 계획을 보내는 경로가 아직 없어서, 시간표는 PC 앱에서 확인하세요.</p>;
  }
  return (
    <ul>
      {schedule.items.items.map((item) => (
        <li key={item.occurrence_key} className={item.status === 'skipped' ? 'struck' : undefined}>
          {new Date(item.start_ms).toLocaleTimeString('ko-KR', { hour: '2-digit', minute: '2-digit' })} {item.plan_name} · {item.subject} · {item.sets}세트
        </li>
      ))}
    </ul>
  );
}
