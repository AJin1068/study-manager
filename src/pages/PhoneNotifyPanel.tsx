import { useEffect, useState } from 'react';
import { commands, type RemoteConfig, type SyncStatus, type NtfySubscription } from '../api';
import { describeOutcome } from '../hooks/useStudyLock';
import { NOTIFY_CHANNELS, TOPIC_WARNING, parseGrace, signalSummary, svgDataUri, testResultMessage } from './notifyView';

interface Props {
  config: RemoteConfig | null;
  topicSet: boolean;
  /** Widening a setting is refused by the core during a planned run; the screen mirrors that. */
  editable: boolean;
  onChanged: () => void;
}

/** 「폰 알림」: channel, topic + QR, grace, test notification, and the signal/sync status. */
export function PhoneNotifyPanel({ config, topicSet, editable, onChanged }: Props) {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [subscription, setSubscription] = useState<NtfySubscription | null>(null);
  const [grace, setGrace] = useState<string | null>(null);
  const [chatId, setChatId] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    const load = () => {
      commands
        .getSyncStatus()
        .then((next) => !disposed && setStatus(next))
        .catch((err: unknown) => !disposed && setError(String(err)));
    };
    load();
    const timer = window.setInterval(load, 10_000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, []);

  const channel = config?.notifyChannel ?? 'none';
  useEffect(() => {
    let disposed = false;
    if (channel !== 'ntfy' || !topicSet) return;
    commands
      .getNtfySubscription()
      .then((next) => !disposed && setSubscription(next))
      .catch((err: unknown) => !disposed && setError(String(err)));
    return () => {
      disposed = true;
    };
  }, [channel, topicSet, config?.notifyNtfyServer]);

  async function run(action: () => Promise<string | null>) {
    try {
      const result = await action();
      setError(null);
      if (result !== null) setMessage(result);
    } catch (err) {
      setError(String(err));
    } finally {
      onChanged();
    }
  }

  const graceText = grace ?? String(config?.notifyMissedGraceMin ?? 3);
  const graceCheck = parseGrace(graceText);
  const summary = status ? signalSummary(status) : null;

  return (
    <section aria-labelledby="phone-notify-title">
      <h3 id="phone-notify-title">폰 알림</h3>
      <p className="hint">계획 시작 시각에 PC가 켜지지 않으면 서버가 폰으로 알립니다. 서버로 가는 것은 계획 시각과 &quot;PC가 켜져 있다&quot;는 신호뿐입니다.</p>
      {error && <p className="error" role="alert">{error}</p>}
      {summary && (
        <div data-tone={summary.tone}>
          <strong>{summary.headline}</strong>
          {summary.details.map((line) => (
            <div key={line} className="hint">{line}</div>
          ))}
        </div>
      )}

      <div className="row">
        <label>
          채널{' '}
          <select
            value={channel}
            disabled={!editable && channel === 'none'}
            onChange={(e) =>
              void run(async () => describeOutcome(await commands.updateRemoteSetting('notifyChannel', e.target.value)))
            }
          >
            {NOTIFY_CHANNELS.map((option) => (
              <option key={option.value} value={option.value}>{option.label}</option>
            ))}
          </select>
        </label>
      </div>

      {channel === 'ntfy' && (
        <>
          <div className="row">
            <button
              type="button"
              disabled={!editable}
              onClick={() => void run(async () => describeOutcome(await commands.generateNtfyTopic()))}
            >
              {topicSet ? '토픽 다시 만들기' : '토픽 만들기'}
            </button>
          </div>
          <p className="warn">{TOPIC_WARNING}</p>
          {channel === 'ntfy' && topicSet && subscription && (
            <div>
              <img src={svgDataUri(subscription.qrSvg)} alt="ntfy 구독 QR 코드" width={200} height={200} />
              <div className="hint">폰에서 ntfy 앱을 설치한 뒤 이 QR을 스캔하거나 아래 주소를 여세요.</div>
              <code>{subscription.deepLink}</code>{' '}
              <button type="button" onClick={() => void navigator.clipboard.writeText(subscription.deepLink).then(() => setMessage('주소를 복사했습니다.'))}>
                복사
              </button>
            </div>
          )}
        </>
      )}

      {channel === 'telegram' && (
        <div className="row">
          <label>
            chat_id{' '}
            <input type="text" value={chatId ?? config?.notifyTelegramChatId ?? ''} disabled={!editable} onChange={(e) => setChatId(e.target.value)} />
          </label>
          <button
            type="button"
            disabled={!editable || chatId === null}
            onClick={() =>
              void run(async () => {
                const outcome = await commands.updateRemoteSetting('notifyTelegramChatId', (chatId ?? '').trim());
                setChatId(null);
                return describeOutcome(outcome);
              })
            }
          >
            저장
          </button>
        </div>
      )}

      <div className="row">
        <label>
          유예(분, 1~3){' '}
          <input type="text" inputMode="numeric" size={3} value={graceText} disabled={!editable} onChange={(e) => setGrace(e.target.value)} />
        </label>
        <button
          type="button"
          disabled={!editable || grace === null || !graceCheck.ok}
          onClick={() =>
            void run(async () => {
              if (!graceCheck.ok) return graceCheck.message;
              const outcome = await commands.updateRemoteSetting('notifyMissedGraceMin', graceCheck.value);
              setGrace(null);
              return describeOutcome(outcome);
            })
          }
        >
          저장
        </button>
      </div>
      {grace !== null && !graceCheck.ok && <p className="error">{graceCheck.message}</p>}

      <div className="row">
        <button
          type="button"
          disabled={channel === 'none'}
          onClick={() =>
            void run(async () => {
              const result = await commands.testNotification();
              return testResultMessage(result.ok, result.code);
            })
          }
        >
          테스트 알림 보내기
        </button>
      </div>
      {message && <p role="status" className="message">{message}</p>}
    </section>
  );
}
