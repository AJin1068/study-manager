import { describe, expect, it } from 'vitest';
import type { ModeConfig } from '../api';
import { draftFromConfig, parseModeConfigDraft } from './modeConfigForm';

const easy: ModeConfig = { level: 'easy', studyMin: 25, breakMin: 5, longBreakMin: 20, setsBeforeLongBreak: 4 };
const hard: ModeConfig = { level: 'hard', studyMin: 90, breakMin: 10, longBreakMin: null, setsBeforeLongBreak: null };

describe('parseModeConfigDraft', () => {
  it('round-trips an unchanged config', () => {
    expect(parseModeConfigDraft('easy', draftFromConfig(easy))).toEqual({ ok: true, config: easy });
    expect(parseModeConfigDraft('hard', draftFromConfig(hard))).toEqual({ ok: true, config: hard });
  });

  it('accepts the inclusive 1 and 240 minute bounds', () => {
    const draft = { ...draftFromConfig(hard), studyMin: '1', breakMin: '240' };
    expect(parseModeConfigDraft('hard', draft)).toMatchObject({ ok: true, config: { studyMin: 1, breakMin: 240 } });
  });

  it.each(['0', '241', '-1', '1.5', 'abc', ''])('rejects %j minutes', (bad) => {
    const draft = { ...draftFromConfig(hard), studyMin: bad };
    expect(parseModeConfigDraft('hard', draft).ok).toBe(false);
    expect(parseModeConfigDraft('hard', { ...draftFromConfig(hard), breakMin: bad }).ok).toBe(false);
  });

  it('requires easy long-break fields and ignores them for other levels', () => {
    expect(parseModeConfigDraft('easy', { ...draftFromConfig(easy), longBreakMin: '' }).ok).toBe(false);
    expect(parseModeConfigDraft('easy', { ...draftFromConfig(easy), setsBeforeLongBreak: '1' }).ok).toBe(false);
    expect(parseModeConfigDraft('easy', { ...draftFromConfig(easy), setsBeforeLongBreak: '13' }).ok).toBe(false);
    const withStray = { ...draftFromConfig(hard), longBreakMin: '999', setsBeforeLongBreak: 'x' };
    expect(parseModeConfigDraft('hard', withStray)).toEqual({ ok: true, config: hard });
  });
});
