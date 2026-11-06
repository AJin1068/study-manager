import { useEffect, useState } from 'react';
import { commands, type MessageConfig, type MessageSettingKey, type MessageTone } from '../api';
import { describeOutcome } from '../hooks/useStudyLock';

const TONES: ReadonlyArray<{ value: MessageTone; label: string }> = [
  { value: 'gentle', label: '1단계 다정함' },
  { value: 'warm', label: '2단계 따뜻함' },
  { value: 'normal', label: '3단계 보통' },
  { value: 'firm', label: '4단계 단호함' },
  { value: 'harsh', label: '5단계 독설' },
];

const TOGGLES: ReadonlyArray<{
  key: MessageSettingKey;
  field: 'enabled' | 'eventsEnabled' | 'randomEnabled' | 'notifyEnabled';
  label: string;
}> = [
  { key: 'enabled', field: 'enabled', label: '공부 중 응원·채찍 메시지 받기' },
  { key: 'eventsEnabled', field: 'eventsEnabled', label: '딴짓 시도, 세트 종료, 연속 달성 때 보내기' },
  { key: 'randomEnabled', field: 'randomEnabled', label: '가끔 무작위로도 보내기' },
  { key: 'notifyEnabled', field: 'notifyEnabled', label: '시스템 알림으로도 보내기 (과목명 없이)' },
];

/** Tone and frequency of the cheer/scold messages. The core refuses loosening changes during a locked block. */
export function MessageSettingsPanel() {
  const [config, setConfig] = useState<MessageConfig | null>(null);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    commands
      .getMessageSettings()
      .then(setConfig)
      .catch((err: unknown) => setNote(`실패: ${String(err)}`));
  }, []);

  async function update(key: MessageSettingKey, value: number | boolean | string) {
    try {
      setNote(describeOutcome(await commands.updateMessageSetting(key, value)));
      setConfig(await commands.getMessageSettings());
    } catch (err) {
      setNote(`실패: ${String(err)}`);
    }
  }

  if (config === null) return note === null ? null : <p className="error">{note}</p>;

  return (
    <section className="panel" aria-label="응원·채찍 설정">
      <h2>응원·채찍 메시지</h2>
      {TOGGLES.map((toggle) => (
        <label key={toggle.key} className="row">
          <input type="checkbox" checked={config[toggle.field]} onChange={(e) => void update(toggle.key, e.target.checked)} />
          {toggle.label}
        </label>
      ))}
      <label className="row">
        말투
        <select value={config.tone} onChange={(e) => void update('tone', e.target.value)}>
          {TONES.map((tone) => (
            <option key={tone.value} value={tone.value}>
              {tone.label}
            </option>
          ))}
        </select>
      </label>
      <label className="row">
        한 시간에 최대
        <input type="number" min={1} max={12} value={config.maxPerHour} onChange={(e) => void update('maxPerHour', Number(e.target.value))} />개
      </label>
      <label className="row">
        무작위 메시지 간격(분)
        <input
          type="number"
          min={10}
          max={120}
          value={config.randomIntervalMin}
          onChange={(e) => void update('randomIntervalMin', Number(e.target.value))}
        />
      </label>
      {note !== null && <p className="hint">{note}</p>}
    </section>
  );
}
