// Pure text helpers of the phone screens.

import type { BlockerStatus, FreeReason, PhoneLock, PhoneView } from './api';

/** `mm:ss`, or `h:mm:ss` from one hour up. Negative input counts as zero. */
export function formatDuration(ms: number): string {
  const total = Math.floor(Math.max(0, ms) / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const two = (n: number) => String(n).padStart(2, '0');
  return h > 0 ? `${h}:${two(m)}:${two(s)}` : `${two(m)}:${two(s)}`;
}

const FREE_TEXT: Record<FreeReason, string> = {
  no_lease: '받은 잠금 없음',
  lease_expired: '잠금 시간이 끝남',
  clock_invalid: '시계를 읽을 수 없음(서버에 다시 확인)',
  run_ended: '공부가 끝남',
  emergency_pass: '긴급 패스',
  skip_today: '오늘 건너뜀',
  early_release: '조기 해제',
  pair_revoked: '짝이 해제됨',
};

export function lockText(lock: PhoneLock): string {
  return lock.state === 'locked' ? '잠금 중' : `풀림: ${FREE_TEXT[lock.reason]}`;
}

export function blockerText(status: BlockerStatus): string {
  switch (status.kind) {
    case 'active':
      return '차단 중';
    case 'inactive':
      return '차단 안 함';
    case 'service_disabled':
      return '접근성 서비스가 꺼져 있음(차단되지 않음)';
    case 'unreachable':
      return '차단기가 응답하지 않음';
    case 'unsupported':
      return '이 기기는 잠글 수 없음(아이폰은 지원하지 않습니다)';
  }
}

const ERROR_TEXT: Record<string, string> = {
  bad_offer: 'PC 앱이 보여 준 짝짓기 QR이 아닙니다.',
  invalid_code: '코드가 맞지 않습니다.',
  used: '이미 쓴 코드입니다. PC에서 새 코드를 만드세요.',
  expired: '코드가 만료되었습니다(5분). PC에서 새 코드를 만드세요.',
  rate_limited: '시도가 너무 많습니다. 잠시 뒤에 다시 하세요.',
  cannot_self: '이 계정으로는 짝지을 수 없습니다.',
  already_paired: 'PC가 이미 다른 폰과 짝지어져 있습니다.',
  phone_already_paired: '이 폰은 이미 짝지어져 있습니다.',
  locked: '잠금 중에는 짝을 끊을 수 없습니다.',
  network: '서버에 닿지 않습니다.',
  timeout: '서버 응답이 늦습니다.',
  auth_expired: '로그인이 만료되었습니다. 다시 짝지으세요.',
  storage: '이 기기에 저장하지 못했습니다.',
};

export function errorText(code: string): string {
  return ERROR_TEXT[code] ?? `실패: ${code}`;
}

/** Locked on the lease but not actually blocked: the user must see that the lock is not working. */
export function lockIsHollow(view: PhoneView): boolean {
  const report = view.report;
  if (!report || report.diagnostics.lock.state !== 'locked') return false;
  return report.blocker.kind !== 'active' && report.blocker.kind !== 'unreachable';
}
