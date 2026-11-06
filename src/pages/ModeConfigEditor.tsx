import { useState } from 'react';
import {
  commands,
  MODE_MINUTES_MAX,
  MODE_MINUTES_MIN,
  SETS_BEFORE_LONG_BREAK_MAX,
  SETS_BEFORE_LONG_BREAK_MIN,
  type ModeConfig,
} from '../api';
import { failureNotice, modeChangeNotice, type ModeNotice } from './modeShiftNotice';
import { draftFromConfig, parseModeConfigDraft, type ModeConfigDraft } from './modeConfigForm';

const LEVEL_LABEL: Record<ModeConfig['level'], string> = { easy: '쉬움', medium: '보통', hard: '어려움' };

interface Props {
  config: ModeConfig;
  /** Controls may be edited: lock state known and free. The Rust core enforces it again. */
  editable: boolean;
  /** Called after any attempt (applied, rejected, or failed) so the parent can re-read state. */
  onAttempted: (notice: ModeNotice) => void;
}

export function ModeConfigEditor({ config, editable, onAttempted }: Props) {
  const [draft, setDraft] = useState<ModeConfigDraft>(() => draftFromConfig(config));
  const [formError, setFormError] = useState<string | null>(null);
  const hasLongBreak = config.level === 'easy';

  function field(name: keyof ModeConfigDraft, label: string, min: number, max: number) {
    return (
      <label className="row">
        {label}
        <input
          type="number"
          inputMode="numeric"
          min={min}
          max={max}
          step={1}
          value={draft[name]}
          disabled={!editable}
          onChange={(e) => setDraft({ ...draft, [name]: e.target.value })}
        />
      </label>
    );
  }

  async function save() {
    const parsed = parseModeConfigDraft(config.level, draft);
    if (!parsed.ok) {
      setFormError(parsed.error);
      return;
    }
    setFormError(null);
    try {
      onAttempted(modeChangeNotice(await commands.updateModeConfig(parsed.config)));
    } catch (err) {
      onAttempted(failureNotice(err));
    }
  }

  return (
    <fieldset className="card" disabled={!editable}>
      <legend>{LEVEL_LABEL[config.level]}</legend>
      {field('studyMin', '공부(분)', MODE_MINUTES_MIN, MODE_MINUTES_MAX)}
      {field('breakMin', '휴식(분)', MODE_MINUTES_MIN, MODE_MINUTES_MAX)}
      {hasLongBreak && field('longBreakMin', '긴 휴식(분)', MODE_MINUTES_MIN, MODE_MINUTES_MAX)}
      {hasLongBreak &&
        field('setsBeforeLongBreak', '긴 휴식까지 세트 수', SETS_BEFORE_LONG_BREAK_MIN, SETS_BEFORE_LONG_BREAK_MAX)}
      {formError && <p className="error" role="alert">{formError}</p>}
      <button type="button" onClick={() => void save()} disabled={!editable}>저장</button>
    </fieldset>
  );
}
