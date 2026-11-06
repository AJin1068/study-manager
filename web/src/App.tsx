import { useEffect, useState } from 'react';
import { ApiError, type WebApi } from './api';
import { parseRoute, summarize, type DashboardData, type ProfileResult, type Route } from './model';
import { DashboardView, LoginForm, ProfileView } from './views';

function todayLocal(): string {
  const now = new Date();
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
}

function useRoute(): Route {
  const [route, setRoute] = useState<Route>(() => parseRoute(window.location.hash));
  useEffect(() => {
    const onChange = () => setRoute(parseRoute(window.location.hash));
    window.addEventListener('hashchange', onChange);
    return () => window.removeEventListener('hashchange', onChange);
  }, []);
  return route;
}

function ProfilePage({ api, slug }: { api: WebApi; slug: string }) {
  const [state, setState] = useState<{ result: ProfileResult } | { error: string } | null>(null);
  useEffect(() => {
    let live = true;
    api
      .publicProfile(slug)
      .then((result) => live && setState({ result }))
      .catch(() => live && setState({ error: '서버에 연결하지 못했습니다.' }));
    return () => {
      live = false;
    };
  }, [api, slug]);
  if (state === null) return <main className="page"><p>불러오는 중</p></main>;
  if ('error' in state) return <main className="page"><p className="error">{state.error}</p></main>;
  return <ProfileView result={state.result} />;
}

function DashboardPage({ api }: { api: WebApi }) {
  const [signed, setSigned] = useState(api.signedIn());
  const [data, setData] = useState<DashboardData | null>(null);
  const [step, setStep] = useState<'email' | 'code'>('email');
  const [email, setEmail] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!signed) return;
    let live = true;
    api
      .dashboard()
      .then((d) => live && setData(d))
      .catch((err: unknown) => {
        if (!live) return;
        if (err instanceof ApiError && err.status === 401) setSigned(false);
        else setError('서버에서 기록을 읽지 못했습니다.');
      });
    return () => {
      live = false;
    };
  }, [api, signed]);

  async function attempt(run: () => Promise<void>, next?: () => void) {
    setBusy(true);
    setError(null);
    try {
      await run();
      next?.();
    } catch {
      setError('처리하지 못했습니다. 입력을 확인하고 다시 시도하세요.');
    } finally {
      setBusy(false);
    }
  }

  if (!signed) {
    return (
      <LoginForm
        step={step}
        busy={busy}
        error={error}
        onEmail={(value) => {
          setEmail(value);
          void attempt(() => api.requestOtp(value), () => setStep('code'));
        }}
        onCode={(code) => void attempt(() => api.verifyOtp(email, code), () => setSigned(true))}
      />
    );
  }
  if (error !== null) return <main className="page"><p className="error">{error}</p></main>;
  if (data === null) return <main className="page"><p>불러오는 중</p></main>;
  return (
    <>
      <DashboardView data={data} summary={summarize(data.days, todayLocal())} />
      <p className="page">
        <button type="button" onClick={() => { api.signOut(); setSigned(false); setData(null); setStep('email'); }}>로그아웃</button>
      </p>
    </>
  );
}

export function App({ api }: { api: WebApi }) {
  const route = useRoute();
  if (route.kind === 'profile') return <ProfilePage api={api} slug={route.slug} />;
  if (route.kind === 'dashboard') return <DashboardPage api={api} />;
  return (
    <main className="page">
      <h1>Study Manager</h1>
      <p>프로필 링크(<code>#/u/…</code>)를 열거나 <a href="#/dashboard">내 대시보드</a>에 로그인하세요.</p>
    </main>
  );
}
