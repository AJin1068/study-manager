import type { DashboardSummary, DashboardData, ProfileResult } from './model';

function grassLevel(min: number): 0 | 1 | 2 | 3 | 4 {
  if (min <= 0) return 0;
  if (min < 30) return 1;
  if (min < 90) return 2;
  if (min < 180) return 3;
  return 4;
}

/** The public profile: only sections the server returned are drawn. */
export function ProfileView({ result }: { result: ProfileResult }) {
  if (result.kind === 'not_found') {
    return (
      <main className="page">
        <p>프로필을 찾을 수 없거나 공개된 내용이 없습니다.</p>
      </main>
    );
  }
  const p = result.profile;
  return (
    <main className="page">
      <h1>{p.name}</h1>
      {p.shown.length === 0 && <p className="hint">공개된 내용이 없습니다.</p>}
      {p.level !== null && <p>{`레벨 ${p.level}`}</p>}
      {p.room !== null && (
        <p>
          {p.room.species ?? '캐릭터'} · {[p.room.hat, p.room.top, p.room.accessory, p.room.hand].filter((x) => x !== null).join(', ') || '맨몸'}
          {p.room.furniture.length > 0 && ` · 가구 ${p.room.furniture.join(', ')}`}
        </p>
      )}
      {p.grass !== null && (
        <div className="grass" role="img" aria-label="공부 잔디">
          {p.grass.map((d) => (
            <span key={d.day} className={`grass-cell grass-${grassLevel(d.studyMin)}`} title={`${d.day} ${d.studyMin}분`} />
          ))}
        </div>
      )}
      {p.badges !== null && <p>배지 {p.badges.length === 0 ? '없음' : p.badges.join(', ')}</p>}
      {p.subject !== null && <p>지금 공부 중: {p.subject}</p>}
      {p.wiki !== null && <p>공개한 위키: {p.wiki.length === 0 ? '없음' : p.wiki.join(', ')}</p>}
      {p.visitors !== null && <p>방문자 오늘 {p.visitors.today} · 전체 {p.visitors.total}</p>}
      {p.gifts !== null && (
        <ul>
          {p.gifts.map((g, i) => (
            <li key={`${g.itemId}-${i}`}>
              {g.senderName}님이 준 {g.itemId}
              {g.message !== '' && ` — ${g.message}`}
            </li>
          ))}
        </ul>
      )}
    </main>
  );
}

export function LoginForm(props: {
  step: 'email' | 'code';
  busy: boolean;
  error: string | null;
  onEmail: (email: string) => void;
  onCode: (code: string) => void;
}) {
  const submit = (read: (data: FormData) => string, send: (value: string) => void) => (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    send(read(new FormData(event.currentTarget)));
  };
  return (
    <main className="page">
      <h1>대시보드 로그인</h1>
      {props.step === 'email' ? (
        <form onSubmit={submit((d) => String(d.get('email') ?? ''), props.onEmail)}>
          <label>
            이메일 <input name="email" type="email" required autoComplete="email" />
          </label>{' '}
          <button type="submit" disabled={props.busy}>인증 코드 받기</button>
        </form>
      ) : (
        <form onSubmit={submit((d) => String(d.get('code') ?? ''), props.onCode)}>
          <label>
            메일로 받은 코드 <input name="code" inputMode="numeric" required autoComplete="one-time-code" />
          </label>{' '}
          <button type="submit" disabled={props.busy}>확인</button>
        </form>
      )}
      {props.error !== null && <p className="error" role="alert">{props.error}</p>}
    </main>
  );
}

/** The signed-in owner's own totals: minutes per day and the weekly achievement. No app names, titles or attempt counts. */
export function DashboardView({ data, summary }: { data: DashboardData; summary: DashboardSummary }) {
  return (
    <main className="page">
      <h1>내 대시보드</h1>
      <p>{[data.level !== null ? `레벨 ${data.level}` : null, data.species].filter((x) => x !== null).join(' · ')}</p>
      <p>{`최근 7일 공부 ${summary.weekStudyMin}분 · 달성률 ${summary.weekAchievementPct === null ? '계획 없음' : `${summary.weekAchievementPct}%`}`}</p>
      {summary.rows.length === 0 ? (
        <p className="hint">서버에 올라간 기록이 없습니다. 앱의 친구 공유에서 「주간 공부 시간」을 켜면 나타납니다.</p>
      ) : (
        <table>
          <thead>
            <tr><th>날짜</th><th>공부(분)</th><th>계획(분)</th></tr>
          </thead>
          <tbody>
            {summary.rows.map((r) => (
              <tr key={r.day}><td>{r.day}</td><td>{r.studyMin}</td><td>{r.planMin}</td></tr>
            ))}
          </tbody>
        </table>
      )}
    </main>
  );
}
