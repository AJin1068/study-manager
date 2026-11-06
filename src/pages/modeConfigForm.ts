import {
  MODE_MINUTES_MAX,
  MODE_MINUTES_MIN,
  SETS_BEFORE_LONG_BREAK_MAX,
  SETS_BEFORE_LONG_BREAK_MIN,
  type ModeConfig,
} from '../api';

/** Raw text of the inputs; empty string means "not entered". */
export interface ModeConfigDraft {
  studyMin: string;
  breakMin: string;
  longBreakMin: string;
  setsBeforeLongBreak: string;
}

export type ModeConfigParse = { ok: true; config: ModeConfig } | { ok: false; error: string };

export function draftFromConfig(config: ModeConfig): ModeConfigDraft {
  return {
    studyMin: String(config.studyMin),
    breakMin: String(config.breakMin),
    longBreakMin: config.longBreakMin === null ? '' : String(config.longBreakMin),
    setsBeforeLongBreak:
      config.setsBeforeLongBreak === null ? '' : String(config.setsBeforeLongBreak),
  };
}

function wholeNumber(raw: string, label: string, min: number, max: number): number | string {
  const text = raw.trim();
  if (!/^\d+$/.test(text)) return `${label}: 정수를 입력하세요.`;
  const value = Number(text);
  return value >= min && value <= max ? value : `${label}: ${min}~${max} 사이로 입력하세요.`;
}

/**
 * Early feedback only: the Rust core validates the same ranges and is the authority.
 * Only easy has a long break (PLAN); other levels send null for both long-break fields.
 */
export function parseModeConfigDraft(level: ModeConfig['level'], draft: ModeConfigDraft): ModeConfigParse {
  const study = wholeNumber(draft.studyMin, '공부(분)', MODE_MINUTES_MIN, MODE_MINUTES_MAX);
  if (typeof study === 'string') return { ok: false, error: study };
  const rest = wholeNumber(draft.breakMin, '휴식(분)', MODE_MINUTES_MIN, MODE_MINUTES_MAX);
  if (typeof rest === 'string') return { ok: false, error: rest };
  if (level !== 'easy') {
    return {
      ok: true,
      config: { level, studyMin: study, breakMin: rest, longBreakMin: null, setsBeforeLongBreak: null },
    };
  }
  const long = wholeNumber(draft.longBreakMin, '긴 휴식(분)', MODE_MINUTES_MIN, MODE_MINUTES_MAX);
  if (typeof long === 'string') return { ok: false, error: long };
  const sets = wholeNumber(
    draft.setsBeforeLongBreak,
    '긴 휴식까지 세트 수',
    SETS_BEFORE_LONG_BREAK_MIN,
    SETS_BEFORE_LONG_BREAK_MAX,
  );
  if (typeof sets === 'string') return { ok: false, error: sets };
  return {
    ok: true,
    config: { level, studyMin: study, breakMin: rest, longBreakMin: long, setsBeforeLongBreak: sets },
  };
}
