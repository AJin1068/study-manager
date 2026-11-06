// Display helpers for the profile screen. The core decides what is visible; the UI only words it.

import type { GrassDay, OutboxState, ProfileCode, Scope, Section } from '../api';

const CODE_TEXT: Record<ProfileCode, string> = {
  ok: '완료했습니다',
  not_signed_in: '먼저 연동 화면에서 로그인하세요',
  invalid_input: '입력을 확인하세요',
  not_shared: '이 칸은 공개로 바꾼 뒤에 쓸 수 있습니다',
  not_found: '찾을 수 없습니다',
  not_member: '같은 방 멤버만 볼 수 있습니다',
  cannot_self: '자기 자신에게는 할 수 없습니다',
  not_friends: '같은 방 멤버에게만 할 수 있습니다',
  closed: '방명록이 닫혀 있습니다',
  rate_limited: '너무 잦습니다. 잠시 후 다시 하세요',
  key_conflict: '이미 처리된 요청입니다',
  not_giftable: '선물할 수 없는 물건입니다 (업적·기본 물건·캐릭터는 불가)',
  daily_limit: '하루 선물 한도에 도달했습니다',
  already_resolved: '이미 받거나 거절한 선물입니다',
  name_required: '프로필 이름을 먼저 입력하세요',
  insufficient_points: '포인트가 부족합니다',
  already_owned: '이미 가지고 있습니다',
  unknown_item: '없는 물건입니다',
  too_many: '너무 많습니다 (위키 페이지는 10개까지)',
};

export function codeText(code: ProfileCode): string {
  return CODE_TEXT[code];
}

export const SECTION_LABEL: Record<Section, string> = {
  room: '내 방 (오두막과 캐릭터)',
  grass: '공부 잔디',
  level: '레벨',
  badges: '업적 배지',
  guestbook: '방명록',
  visitors: '방문자 수',
  wiki: '공개한 위키 페이지 이름',
  subject: '지금 공부 중인 과목',
  gifts: '받은 선물',
};

export const SCOPE_LABEL: Record<Scope, string> = {
  public: '전체 (링크를 가진 누구나)',
  friends: '친구만 (같은 방 멤버)',
  private: '비공개',
};

export const PRIVACY_NOTICE =
  '모든 칸은 처음에 비공개입니다. 딴짓 횟수·앱 이름·창 제목·포인트는 어떤 설정에서도 공개되지 않습니다.';

export const LINK_NOTICE = '이 링크를 아는 사람은 「전체」로 연 칸을 로그인 없이 볼 수 있습니다. 링크는 본인에게만 보여 줍니다.';

export function scopeChangeText(section: Section, scope: Scope): string {
  return scope === 'private'
    ? `${SECTION_LABEL[section]}: 비공개로 바꾸고 서버에 있던 내용을 지웁니다`
    : `${SECTION_LABEL[section]}: ${SCOPE_LABEL[scope]}`;
}

const STATE_TEXT: Record<OutboxState, string> = {
  pending: '확인 중 (환불하지 않고 서버에 물어보는 중)',
  sent: '전달됨',
  refunded: '전달 실패 · 포인트 환불됨',
};

export function outboxStateText(state: OutboxState): string {
  return STATE_TEXT[state];
}

/** 0 = no study, 1-4 = darker with more minutes. */
export function grassLevel(studyMin: number): 0 | 1 | 2 | 3 | 4 {
  if (studyMin <= 0) return 0;
  if (studyMin < 30) return 1;
  if (studyMin < 90) return 2;
  if (studyMin < 180) return 3;
  return 4;
}

export interface GrassCell {
  day: string;
  level: 0 | 1 | 2 | 3 | 4;
  studyMin: number;
}

function isoDay(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}

/** The last `days` calendar days ending at `today` (YYYY-MM-DD), oldest first; days with no row are level 0. */
export function grassCells(days: readonly GrassDay[], today: string, count = 365): GrassCell[] {
  const byDay = new Map(days.map((d) => [d.day, d.studyMin]));
  const end = Date.parse(`${today}T00:00:00Z`);
  if (Number.isNaN(end)) return [];
  const cells: GrassCell[] = [];
  for (let i = count - 1; i >= 0; i -= 1) {
    const day = isoDay(end - i * 86_400_000);
    const studyMin = byDay.get(day) ?? 0;
    cells.push({ day, level: grassLevel(studyMin), studyMin });
  }
  return cells;
}

export function totalMinutes(days: readonly GrassDay[]): number {
  return days.reduce((sum, d) => sum + d.studyMin, 0);
}

/** 「민수가 준 목도리」 */
export function giftLine(senderName: string, itemLabel: string): string {
  return `${senderName}님이 준 ${itemLabel}`;
}
