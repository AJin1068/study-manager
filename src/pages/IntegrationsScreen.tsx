import { useEffect, useState } from 'react';
import {
  commands,
  onRemoteState,
  type IntegrationPort,
  type ModeSetting,
  type SecretName,
} from '../api';
import { describeOutcome } from '../hooks/useStudyLock';
import { useAsync } from '../hooks/useAsync';
import { PhoneNotifyPanel } from './PhoneNotifyPanel';
import {
  PORT_LABELS,
  authSummary,
  connectionMessage,
  draftAfterSave,
  eventKindLabel,
  formatClock,
  healthText,
  modeBadge,
  saveSecretMessage,
  secretStoreNotice,
  verifyMessage,
} from './integrationView';

interface Props {
  /** Entering or widening settings is allowed: lock known and free. Tightening stays enabled; the core decides. */
  editable: boolean;
  onAttempted: () => void;
}

const SECRET_FIELDS: { name: SecretName; label: string; flag: 'anthropicApiKey' | 'ntfyTopic' }[] = [
  { name: 'anthropic_api_key', label: 'Anthropic API 키', flag: 'anthropicApiKey' },
  { name: 'ntfy_topic', label: 'ntfy 토픽 (비밀번호처럼 취급)', flag: 'ntfyTopic' },
];

/** 「연동」: mode badges, keys, sign-in, and what has left the device. */
export function IntegrationsScreen({ editable, onAttempted }: Props) {
  const states = useAsync(commands.getIntegrationStates);
  const config = useAsync(commands.getRemoteSettings);
  const summary = useAsync(commands.getEgressSummary);
  const [endpoint, setEndpoint] = useState<string | null>(null);
  const [anonKey, setAnonKey] = useState<string | null>(null);
  const [secretDrafts, setSecretDrafts] = useState<Record<string, string>>({});
  const [email, setEmail] = useState('');
  const [code, setCode] = useState('');
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const reloadStates = states.reload;
  const reloadSummary = summary.reload;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onRemoteState(() => {
      reloadStates();
      reloadSummary();
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [reloadStates, reloadSummary]);

  async function run(action: () => Promise<string | null>) {
    try {
      const result = await action();
      setError(null);
      if (result !== null) setMessage(result);
    } catch (err) {
      setError(String(err));
    } finally {
      states.reload();
      config.reload();
      summary.reload();
      onAttempted();
    }
  }

  const view = states.data;
  const stored = config.data;
  const notice = view ? secretStoreNotice(view.secretStore) : null;

  return (
    <section className="card" aria-labelledby="integrations-title">
      <h2 id="integrations-title">연동</h2>
      <p className="hint">
        서버·알림·LLM은 설정하기 전까지 꺼져 있고, 꺼져 있어도 타이머·차단·계획은 그대로 동작합니다.
        &quot;모의 모드&quot;는 외부로 아무것도 보내지 않는 연습용입니다.
      </p>
      {states.error && <p className="error">연동 상태를 읽지 못했습니다: {states.error}</p>}
      {error && <p className="error" role="alert">{error}</p>}
      {notice && <p className="warn" role="alert">{notice}</p>}

      <ul className="list">
        {(view?.ports ?? []).map((state) => {
          const badge = modeBadge(state);
          const health = healthText(state.health);
          return (
            <li key={state.port}>
              <strong>{PORT_LABELS[state.port]}</strong> <span data-tone={badge.tone}>[{badge.label}]</span>
              {state.lastOkMs !== null && <span className="hint"> 마지막 성공 {formatClock(state.lastOkMs)}</span>}
              {health && <div className="warn">{health}</div>}
              <div className="row">
                <label>
                  <input
                    type="radio"
                    name={`mode-${state.port}`}
                    checked={state.setting === 'auto'}
                    disabled={!editable}
                    onChange={() => void run(async () => describeOutcome(await commands.setIntegrationMode(state.port, 'auto' satisfies ModeSetting)))}
                  />{' '}
                  자동
                </label>
                <label>
                  <input
                    type="radio"
                    name={`mode-${state.port}`}
                    checked={state.setting === 'mock'}
                    onChange={() => void run(async () => describeOutcome(await commands.setIntegrationMode(state.port, 'mock' satisfies ModeSetting)))}
                  />{' '}
                  모의
                </label>
                <button
                  type="button"
                  disabled={state.mode === 'notConfigured'}
                  onClick={() =>
                    void run(async () => {
                      const result = await commands.testConnection(state.port as IntegrationPort);
                      return connectionMessage(result.ok, result.code);
                    })
                  }
                >
                  연결 시험
                </button>
              </div>
            </li>
          );
        })}
      </ul>

      <h3>서버 (Supabase)</h3>
      <div className="row">
        <label>
          주소{' '}
          <input
            type="url"
            value={endpoint ?? stored?.supabaseEndpoint ?? ''}
            disabled={!editable}
            placeholder="https://xxxx.supabase.co"
            onChange={(e) => setEndpoint(e.target.value)}
          />
        </label>
      </div>
      <div className="row">
        <label>
          공개 키(anon){' '}
          <input type="text" value={anonKey ?? stored?.supabaseAnonKey ?? ''} disabled={!editable} onChange={(e) => setAnonKey(e.target.value)} />
        </label>
      </div>
      <p className="hint">공개 키는 비밀이 아닙니다. 데이터는 서버의 행 단위 접근 규칙(RLS)이 지킵니다. service_role 키는 절대 넣지 마세요.</p>
      <button
        type="button"
        disabled={!editable || (endpoint === null && anonKey === null)}
        onClick={() =>
          void run(async () => {
            const outcomes = [];
            if (endpoint !== null) outcomes.push(await commands.updateRemoteSetting('supabaseEndpoint', endpoint.trim()));
            if (anonKey !== null) outcomes.push(await commands.updateRemoteSetting('supabaseAnonKey', anonKey.trim()));
            setEndpoint(null);
            setAnonKey(null);
            const refused = outcomes.find((outcome) => outcome.outcome === 'rejectedLocked');
            return describeOutcome(refused ?? { outcome: 'applied', key: 'supabase' });
          })
        }
      >
        서버 설정 저장
      </button>

      <h3>로그인 (이메일 코드)</h3>
      <p>{view ? authSummary(view.auth) : '확인 중'}</p>
      <div className="row">
        <input type="email" value={email} placeholder="이메일" onChange={(e) => setEmail(e.target.value)} />
        <button
          type="button"
          disabled={email.trim() === ''}
          onClick={() => void run(async () => { await commands.authRequestOtp(email); return '코드를 보냈습니다. 메일을 확인하세요.'; })}
        >
          코드 받기
        </button>
      </div>
      <div className="row">
        <input type="text" inputMode="numeric" maxLength={6} value={code} placeholder="6자리 코드" onChange={(e) => setCode(e.target.value)} />
        <button
          type="button"
          disabled={email.trim() === '' || code.length !== 6}
          onClick={() =>
            void run(async () => {
              const outcome = await commands.authVerifyOtp(email, code);
              setCode('');
              return verifyMessage(outcome, Date.now());
            })
          }
        >
          로그인
        </button>
        <button type="button" disabled={!view?.auth.signedIn && view?.auth.degradedCode === null} onClick={() => void run(async () => { await commands.authSignOut(); return '로그아웃했습니다.'; })}>
          로그아웃
        </button>
      </div>

      <PhoneNotifyPanel
        config={stored ?? null}
        topicSet={view?.secretStore.configured.ntfyTopic ?? false}
        editable={editable}
        onChanged={() => {
          states.reload();
          config.reload();
          onAttempted();
        }}
      />

      <h3>키</h3>
      {SECRET_FIELDS.map((field) => {
        const isSet = view?.secretStore.configured[field.flag] ?? false;
        return (
          <div className="row" key={field.name}>
            <label>
              {field.label}{' '}
              <input
                type="password"
                autoComplete="off"
                value={secretDrafts[field.name] ?? ''}
                placeholder={isSet ? '저장됨 (값은 표시하지 않음)' : '없음'}
                disabled={!editable}
                onChange={(e) => setSecretDrafts({ ...secretDrafts, [field.name]: e.target.value })}
              />
            </label>
            <button
              type="button"
              disabled={!editable || (secretDrafts[field.name] ?? '') === ''}
              onClick={() =>
                void run(async () => {
                  const value = secretDrafts[field.name] ?? '';
                  setSecretDrafts({ ...secretDrafts, [field.name]: draftAfterSave() });
                  return saveSecretMessage(await commands.saveSecret(field.name, value));
                })
              }
            >
              저장
            </button>
            <button type="button" disabled={!isSet} onClick={() => void run(async () => { await commands.clearSecret(field.name); return '삭제했습니다.'; })}>
              삭제
            </button>
          </div>
        );
      })}

      <h3>내보낸 내용</h3>
      <p className="hint">종류와 횟수만 기록합니다. 내용(창 제목, 주소, 입력한 문장)은 기록에도, 전송에도 없습니다.</p>
      {(summary.data ?? []).length === 0 && <p>아직 내보낸 것이 없습니다.</p>}
      <ul className="list">
        {(summary.data ?? []).map((row) => (
          <li key={`${row.port}:${row.kind}`}>
            {eventKindLabel(row.kind)} · {row.count}회 · 마지막 {formatClock(row.lastAtMs)}
          </li>
        ))}
      </ul>
      {message && <p role="status" className="message">{message}</p>}
    </section>
  );
}
