// Display and form helpers for browser blocking. The Rust core validates and decides; these only
// turn text areas into lists and link/coverage state into short Korean sentences.

import type { Coverage, LinkStatus, PresetCategory, SiteRulesConfig } from '../api';

/** One entry per line (commas also separate), trimmed, empty lines dropped. */
export function parseLines(text: string): string[] {
  return text
    .split(/[\n,]/)
    .map((line) => line.trim())
    .filter((line) => line !== '');
}

export function joinLines(items: string[]): string {
  return items.join('\n');
}

export interface SiteRulesDraft {
  strict: boolean;
  /** `false` keeps the built-in strict set (`allow: null`). */
  customAllow: boolean;
  allowText: string;
  playlistsText: string;
  channelsText: string;
}

export function draftFromConfig(config: SiteRulesConfig): SiteRulesDraft {
  return {
    strict: config.strict,
    customAllow: config.allow !== null,
    allowText: joinLines(config.allow ?? []),
    playlistsText: joinLines(config.youtubePlaylists),
    channelsText: joinLines(config.youtubeChannels),
  };
}

export function configFromDraft(draft: SiteRulesDraft): SiteRulesConfig {
  return {
    strict: draft.strict,
    allow: draft.customAllow ? parseLines(draft.allowText) : null,
    youtubePlaylists: parseLines(draft.playlistsText),
    youtubeChannels: parseLines(draft.channelsText),
  };
}

/** The choice map to send: only categories whose switch differs from what is saved. */
export function presetChoice(categories: PresetCategory[], draft: Record<string, boolean>): Record<string, boolean> {
  const choice: Record<string, boolean> = {};
  for (const category of categories) {
    const wanted = draft[category.id] ?? category.enabled;
    if (wanted !== category.enabled) choice[category.id] = wanted;
  }
  return choice;
}

export function presetDraftChanged(categories: PresetCategory[], draft: Record<string, boolean>): boolean {
  return Object.keys(presetChoice(categories, draft)).length > 0;
}

export interface LinkNotice {
  level: 'ok' | 'warn';
  text: string;
}

/**
 * What to tell the user about the extension link. While blocking is in force and no extension is
 * connected, sites load freely (fail-open), and the sentence says so instead of staying quiet.
 */
export function linkNotice(status: LinkStatus, blockingActive: boolean): LinkNotice {
  if (!status.transportActive) {
    return {
      level: 'warn',
      text: `브라우저 연결을 시작하지 못해 사이트 차단이 꺼져 있습니다${status.transportError ? `: ${status.transportError}` : ''}`,
    };
  }
  if (status.connected) return { level: 'ok', text: '브라우저 확장 프로그램이 연결되어 있습니다.' };
  if (blockingActive) {
    return {
      level: 'warn',
      text: '공부 중인데 브라우저 확장 프로그램이 연결되어 있지 않아 사이트가 차단되지 않습니다. 이 시간은 "감시 불가 구간"으로 기록됩니다.',
    };
  }
  return { level: 'warn', text: '브라우저 확장 프로그램이 연결되어 있지 않습니다. 설치하지 않았거나 비활성화/삭제된 상태일 수 있습니다.' };
}

export function coverageText(coverage: Coverage): string {
  const minutes = (ms: number) => Math.round(ms / 60000);
  if (coverage.unmonitoredMs <= 0) return '브라우저 감시 불가 구간 없음';
  return `브라우저 감시 불가 구간 ${minutes(coverage.unmonitoredMs)}분 / 공부 ${minutes(coverage.studyMs)}분`;
}
