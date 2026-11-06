import { describe, expect, it } from 'vitest';
import type { LinkStatus, PresetCategory } from '../api';
import {
  configFromDraft,
  coverageText,
  draftFromConfig,
  linkNotice,
  parseLines,
  presetChoice,
  presetDraftChanged,
} from './browserView';

const status: LinkStatus = {
  transportActive: true,
  transportError: null,
  connected: true,
  connections: 1,
  lastConnectedAtMs: 1,
  lastDisconnectedAtMs: null,
  lastDisconnectReason: null,
};

function category(id: string, enabled: boolean): PresetCategory {
  return { id, label: id, defaultOn: true, enabled, apps: [], sites: [] };
}

describe('site rule form', () => {
  it('splits lines and commas, trims, and drops empties', () => {
    expect(parseLines(' a.com\n\n b.com , c.com ,')).toEqual(['a.com', 'b.com', 'c.com']);
    expect(parseLines('')).toEqual([]);
  });

  it('keeps the built-in strict set unless the custom list is switched on', () => {
    const draft = draftFromConfig({ strict: true, allow: null, youtubePlaylists: [], youtubeChannels: [] });
    expect(draft.customAllow).toBe(false);
    expect(configFromDraft({ ...draft, allowText: 'ignored.com' }).allow).toBeNull();
    expect(configFromDraft({ ...draft, customAllow: true, allowText: 'a.com\nb.com' }).allow).toEqual(['a.com', 'b.com']);
    // An empty custom list stays empty rather than falling back to the built-in set.
    expect(configFromDraft({ ...draft, customAllow: true, allowText: '' }).allow).toEqual([]);
  });

  it('round-trips a saved config', () => {
    const config = { strict: true, allow: ['github.com'], youtubePlaylists: ['PL1'], youtubeChannels: ['@a', 'UC1'] };
    expect(configFromDraft(draftFromConfig(config))).toEqual(config);
  });
});

describe('preset choice', () => {
  const categories = [category('sns', true), category('game', false)];

  it('sends only the switches that changed', () => {
    expect(presetChoice(categories, {})).toEqual({});
    expect(presetChoice(categories, { sns: false, game: false })).toEqual({ sns: false });
    expect(presetDraftChanged(categories, { game: true })).toBe(true);
    expect(presetDraftChanged(categories, { sns: true })).toBe(false);
  });
});

describe('link notice', () => {
  it('says plainly that sites are not blocked when the link is gone during study', () => {
    expect(linkNotice(status, true).level).toBe('ok');
    const gone = linkNotice({ ...status, connected: false, connections: 0 }, true);
    expect(gone.level).toBe('warn');
    expect(gone.text).toContain('차단되지 않습니다');
  });

  it('reports a transport that never started, with its reason', () => {
    const notice = linkNotice({ ...status, transportActive: false, transportError: 'no listener', connected: false }, false);
    expect(notice.level).toBe('warn');
    expect(notice.text).toContain('no listener');
  });

  it('formats coverage', () => {
    expect(coverageText({ studyMs: 3_000_000, unmonitoredMs: 600_000 })).toBe('브라우저 감시 불가 구간 10분 / 공부 50분');
    expect(coverageText({ studyMs: 3_000_000, unmonitoredMs: 0 })).toBe('브라우저 감시 불가 구간 없음');
  });
});
