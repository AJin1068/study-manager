// Display helpers for the rooms screen. The core computes every value; the UI only words it.

import type { FriendSettingKey, FriendSettings, ImportOutcome, MemberView, RankRow, RoomCode, TogetherItem } from '../api';
import { durationText } from './dashboardView';

const CODE_TEXT: Record<RoomCode, string> = {
  ok: '완료했습니다',
  not_signed_in: '먼저 연동 화면에서 로그인하세요',
  invalid_input: '입력을 확인하세요 (이름 1-40자, 별명 1-20자)',
  room_limit: '직접 만든 방은 5개까지입니다',
  not_owner: '방장만 할 수 있습니다',
  too_many_invites: '유효한 초대가 너무 많습니다. 하나를 취소하세요',
  not_found: '찾을 수 없습니다',
  rate_limited: '시도가 너무 잦습니다. 잠시 후 다시 하세요',
  invalid_code: '초대 코드가 맞지 않습니다',
  revoked: '취소된 초대입니다',
  expired: '기간이 지난 초대입니다',
  exhausted: '사용 횟수를 모두 쓴 초대입니다',
  banned: '이 방에는 들어갈 수 없습니다',
  already_member: '이미 들어가 있는 방입니다',
  room_full: '방이 가득 찼습니다 (최대 8명)',
  not_member: '이 방의 멤버가 아닙니다',
  cannot_kick_self: '자기 자신은 내보낼 수 없습니다',
  not_shared: '시간 공유가 꺼져 있어 보낼 수 없습니다',
  surge_rejected: '갑자기 늘어난 시간은 받지 않습니다',
  disabled: '친구 알림이 꺼져 있습니다',
  throttled: '잠시 후 다시 시도됩니다',
};

export function codeText(code: RoomCode): string {
  return CODE_TEXT[code];
}

/** Why a poll failed, in words. Unknown codes show as a generic connection problem. */
export function degradedText(code: string | null): string | null {
  if (code === null) return null;
  if (code === 'not_signed_in' || code === 'auth_expired') return '로그인이 필요합니다. 마지막으로 받은 내용을 보여 드립니다';
  if (code === 'not_member') return '이 방에서 나가졌습니다';
  return '서버에 연결하지 못했습니다. 마지막으로 받은 내용을 보여 드립니다';
}

export function presenceText(member: Pick<MemberView, 'presence'>): string {
  const { kind, remainingSec } = member.presence;
  const left = remainingSec === null ? '' : ` · ${Math.ceil(remainingSec / 60)}분 남음`;
  switch (kind) {
    case 'studying':
      return `공부 중${left}`;
    case 'break':
      return `쉬는 중${left}`;
    case 'idle':
      return '대기';
    case 'offline':
      return '오프라인';
    case 'hidden':
      return '비공개';
  }
}

export function levelText(level: number | null): string {
  return level === null ? '레벨 비공개' : `Lv.${level}`;
}

export function attemptsText(count: number | null): string | null {
  return count === null ? null : `오늘 시도 ${count}회`;
}

export function rankText(row: RankRow): string {
  if (row.minutes === null || row.rank === null) return `${row.nickname} · 비공개`;
  return `${row.rank}위 ${row.nickname} · ${durationText(row.minutes * 60_000)}`;
}

export function inviteLeft(expiresAtMs: number, nowMs: number): string {
  const left = expiresAtMs - nowMs;
  if (left <= 0) return '만료됨';
  return `${durationText(left)} 남음`;
}

export const IMPORT_TEXT: Record<ImportOutcome, string> = {
  applied: '내 계획에 넣었습니다',
  alreadyImported: '이미 넣은 블록입니다',
  inPast: '이미 시작 시각이 지났습니다',
  notShared: '방장이 더 이상 공유하지 않는 블록입니다',
  rejectedLocked: '공부 중에는 계획을 바꿀 수 없습니다',
  invalid: '다른 계획과 겹치거나 넣을 수 없습니다',
};

/** Blocks that can still be copied: in the future and not copied before. */
export function importable(items: TogetherItem[]): TogetherItem[] {
  return items.filter((item) => !item.inPast && !item.alreadyImported);
}

export interface SettingRow {
  key: FriendSettingKey;
  label: string;
  hint: string;
}

/** Order of the friends card. Everything starts off except level and state. */
export const SETTING_ROWS: SettingRow[] = [
  { key: 'shareLevel', label: '레벨 공개', hint: '친구에게 내 레벨(과 입은 캐릭터)을 보여 줍니다' },
  { key: 'shareStatus', label: '공부 상태 공개', hint: '공부 중 / 쉬는 중 / 대기와 남은 시간만 보여 줍니다' },
  { key: 'shareWeekTime', label: '이번 주 공부 시간 공개', hint: '날짜별 분 단위 합계가 랭킹에 쓰입니다' },
  { key: 'shareAttempts', label: '오늘 시도 횟수 공개', hint: '막힌 시도의 횟수만 보냅니다 (무엇인지는 보내지 않습니다)' },
  { key: 'distractionPublish', label: '딴짓 알림 보내기', hint: '시도할 때 친구에게 별명만 알립니다. 앱·사이트·횟수는 보내지 않습니다' },
  { key: 'distractionReceive', label: '친구의 딴짓 알림 받기', hint: '켠 뒤에 생긴 알림부터 받습니다' },
];

/** Studying and the settings lock: the core only lets a share be switched off during a run. */
export function canTurnOn(settings: FriendSettings, key: FriendSettingKey, studying: boolean): boolean {
  return !settings[key] ? !studying : true;
}

export const SHARING_NOTICE = [
  '이 방의 멤버에게 보이는 것: 별명, 레벨, 공부 중/쉬는 중 같은 상태와 남은 시간, 입은 캐릭터.',
  '설정에서 켜면 날짜별 공부 시간과 오늘 시도 횟수도 보입니다 (기본은 꺼짐).',
  '창 제목, 주소, 앱 이름, 입력한 글, 위키·퀴즈 내용, 성격 유형, 생년, 성별은 어떤 경우에도 보내지 않습니다.',
  '목표를 지키지 못한 통계(이탈 기록)는 이 PC에만 있고 방에 보이지 않습니다.',
  '방장은 멤버를 내보낼 수 있고, 내보내진 사람은 다시 들어올 수 없습니다.',
];

export function memberSortKey(member: MemberView): number {
  if (member.isSelf) return 0;
  switch (member.presence.kind) {
    case 'studying':
      return 1;
    case 'break':
      return 2;
    case 'idle':
      return 3;
    case 'offline':
      return 4;
    case 'hidden':
      return 5;
  }
}
