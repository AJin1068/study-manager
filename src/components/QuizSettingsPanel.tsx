import { useEffect, useState } from 'react';
import { commands, type QuizConfig, type QuizSettingKey } from '../api';
import { describeOutcome } from '../hooks/useStudyLock';

const TOGGLES: ReadonlyArray<{ key: QuizSettingKey; field: 'distractionEnabled' | 'checkinEnabled' | 'sessionEndEnabled'; label: string }> = [
  { key: 'distractionEnabled', field: 'distractionEnabled', label: '차단된 앱을 열면 복습 문제 하나 보여주기 (차단은 그대로 유지)' },
  { key: 'checkinEnabled', field: 'checkinEnabled', label: '집중 확인 때 문제 함께 보여주기' },
  { key: 'sessionEndEnabled', field: 'sessionEndEnabled', label: '공부가 끝나면 마무리 퀴즈 보여주기' },
];

/** Quiz switches and the early-release limits. The core refuses changes while a study block is locked. */
export function QuizSettingsPanel() {
  const [config, setConfig] = useState<QuizConfig | null>(null);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    commands
      .getQuizSettings()
      .then(setConfig)
      .catch((err: unknown) => setNote(`실패: ${String(err)}`));
  }, []);

  async function update(key: QuizSettingKey, value: number | boolean) {
    try {
      const outcome = await commands.updateQuizSetting(key, value);
      setNote(describeOutcome(outcome));
      setConfig(await commands.getQuizSettings());
    } catch (err) {
      setNote(`실패: ${String(err)}`);
    }
  }

  if (config === null) return note === null ? null : <p className="error">{note}</p>;

  return (
    <section className="panel" aria-label="퀴즈 설정">
      <h2>퀴즈</h2>
      {TOGGLES.map((toggle) => (
        <label key={toggle.key} className="row">
          <input type="checkbox" checked={config[toggle.field]} onChange={(e) => void update(toggle.key, e.target.checked)} />
          {toggle.label}
        </label>
      ))}
      <label className="row">
        조기 해제 대기 시간(분)
        <input
          type="number"
          min={1}
          value={config.releaseCooldownMin}
          onChange={(e) => void update('releaseCooldownMin', Number(e.target.value))}
        />
      </label>
      <label className="row">
        조기 해제 하루 시도 횟수
        <input
          type="number"
          min={1}
          value={config.releaseMaxAttemptsPerDay}
          onChange={(e) => void update('releaseMaxAttemptsPerDay', Number(e.target.value))}
        />
      </label>
      {note !== null && <p className="hint">{note}</p>}
    </section>
  );
}
