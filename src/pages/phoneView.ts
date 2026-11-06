// Display helpers for the phone pairing card. The core decides; the UI only words it.

import type { GapKind, PairStatusView, PhoneCode, PhoneLinkView } from '../api';

const CODE_TEXT: Record<PhoneCode, string> = {
  ok: '완료했습니다',
  not_signed_in: '먼저 연동 화면에서 로그인하세요',
  invalid_input: '입력을 확인하세요',
  anonymous_not_allowed: '폰 계정으로는 코드를 만들 수 없습니다',
  already_paired: '이미 폰이 연결되어 있습니다. 먼저 연결을 끊으세요',
  rate_limited: '너무 잦습니다. 잠시 후 다시 하세요',
  invalid_code: '코드가 맞지 않습니다',
  used: '이미 사용한 코드입니다. 새 코드를 만드세요',
  expired: '코드가 만료되었습니다. 새 코드를 만드세요',
  cannot_self: '자기 자신과는 연결할 수 없습니다',
  phone_already_paired: '이 폰은 이미 다른 계정과 연결되어 있습니다',
  not_found: '연결된 폰이 없습니다',
  not_paired: '연결되어 있지 않습니다',
  stale_seq: '오래된 신호입니다',
  lease_too_long: '잠금 시간이 너무 깁니다',
  run_locked: '계획된 공부 중에는 폰 연결을 끊을 수 없습니다 (잠금이 약해지기 때문입니다)',
  bad_offer: 'QR 코드를 만들지 못했습니다',
};

export function phoneCodeText(code: PhoneCode): string {
  return CODE_TEXT[code];
}

export const PAIR_NOTICE =
  'QR에는 5분짜리 1회용 코드, 서버 주소, 공개 키만 들어 있습니다. 로그인 정보나 비밀은 들어 있지 않습니다. 폰은 이 PC의 폰 잠금 신호와 폰 상태(연결 시각, 서비스 켜짐 여부, 화면 켠 횟수)만 주고받으며 앱 이름이나 사용 내역은 보내지 않습니다.';

export const REVOKE_NOTICE =
  '연결을 끊으면 폰은 다음 확인 때(최대 15초, 오프라인이면 잠금 시간이 끝날 때) 풀립니다. 긴급 전화는 언제나 가능합니다.';

/** Seconds left on a code, never negative. */
export function codeSecondsLeft(expiresAtMs: number | null, nowMs: number): number {
  if (expiresAtMs === null) return 0;
  return Math.max(0, Math.ceil((expiresAtMs - nowMs) / 1000));
}

export function countdownText(seconds: number): string {
  if (seconds <= 0) return '만료됨';
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return `${m}:${String(s).padStart(2, '0')} 남음`;
}

export interface PairSummary {
  headline: string;
  tone: 'ok' | 'warn' | 'idle';
  details: string[];
}

/** One line for the card header plus warnings (service switched off, server unreachable). */
export function pairSummary(status: PairStatusView, nowMs: number): PairSummary {
  if (!status.paired) {
    return { headline: '연결된 폰 없음', tone: 'idle', details: status.online ? [] : ['서버에 연결하지 못해 마지막으로 알던 상태입니다.'] };
  }
  const details: string[] = [];
  let tone: PairSummary['tone'] = 'ok';
  if (status.serviceEnabled === false) {
    tone = 'warn';
    details.push('폰의 차단 서비스가 꺼져 있습니다. 폰에서 다시 켜세요. 이 시간은 집중 통계에 「폰 미연결」로 남습니다.');
  }
  if (status.lastBeatMs === null) {
    details.push('아직 폰의 신호를 받지 못했습니다.');
  } else {
    const agoSec = Math.max(0, Math.round((nowMs - status.lastBeatMs) / 1000));
    details.push(agoSec < 90 ? '폰의 신호가 정상입니다.' : `폰의 마지막 신호가 ${Math.round(agoSec / 60)}분 전입니다.`);
    if (agoSec >= 90) tone = 'warn';
  }
  if (!status.online) details.push('서버에 연결하지 못해 마지막으로 알던 상태입니다.');
  return { headline: '폰 연결됨', tone, details };
}

const GAP_TEXT: Record<GapKind, string> = {
  unreachable: '폰의 신호가 끊겼습니다. 폰이 켜져 있고 인터넷에 연결되어 있는지 확인하세요.',
  lease_expired: '폰에 잠금을 전하지 못한 채 시간이 지나고 있습니다. 폰이 잠기지 않았을 수 있습니다.',
  service_disabled: '폰의 차단 서비스가 꺼져 있습니다. 폰에서 다시 켜세요.',
};

const GAP_NOTE = '이 시간은 집중 통계에 「폰 미연결 시간」으로 따로 남습니다(점수에는 영향이 없습니다).';

/**
 * Warnings for a study segment that should have locked the phone, in order of urgency. Empty when
 * nothing is paired or nothing is wrong. The core decides; this only words it.
 */
export function phoneLinkWarnings(view: PhoneLinkView, nowMs: number): string[] {
  if (!view.paired) return [];
  const out: string[] = [];
  if (view.wantLocked && view.publishFailing) {
    out.push('서버에 연결하지 못해 폰 잠금 신호를 보내지 못하고 있습니다. PC의 타이머와 차단은 그대로 동작합니다.');
  }
  for (const gap of view.openGaps) {
    const minutes = Math.max(0, Math.floor((nowMs - gap.sinceMs) / 60_000));
    out.push(`${GAP_TEXT[gap.kind]} (${minutes}분째)`);
  }
  if (out.length > 0) out.push(GAP_NOTE);
  return out;
}
