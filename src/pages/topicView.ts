import type { TopicNotice, TopicReason, TopicSource } from '../api';

const NOTICES: Record<TopicNotice, string> = {
  no_plan: '오늘 남은 계획이 없습니다.',
  no_wiki: '이 계획의 과목에는 위키가 없습니다. 위키 화면에서 과목을 만들면 오늘 주제가 여기에 나옵니다.',
  no_pages: '위키에 아직 페이지가 없습니다. 자료를 넣으면 주제를 정해 드립니다.',
};

const SOURCES: Record<TopicSource, string> = {
  assigned: '배정된 주제',
  name_match: '계획 이름과 같은 페이지',
  weak: '보강이 필요한 페이지',
};

const REASONS: Record<TopicReason, string> = {
  empty: '내용이 비어 있음',
  low_accuracy: '자주 틀림',
  stale: '오래 안 봄',
  hard: '집중이 잘 되는 시간대라 어려운 주제',
  review: '복습',
  chosen: '직접 고름',
};

export function noticeText(notice: TopicNotice): string {
  return NOTICES[notice];
}

export function sourceLabel(source: TopicSource): string {
  return SOURCES[source];
}

export function reasonLabel(reason: TopicReason | null): string {
  return reason === null ? '' : REASONS[reason];
}

/** The panel shows the start of a long page; the rest is in the wiki screen. */
export function excerpt(markdown: string, maxChars: number): string {
  const chars = Array.from(markdown.trim());
  return chars.length <= maxChars ? chars.join('') : `${chars.slice(0, maxChars).join('')}…`;
}
