import type {
  AuthStatus,
  Health,
  IntegrationPort,
  IntegrationState,
  SecretStoreStatus,
  UpdateOutcome,
  VerifyOutcome,
} from '../api';

export const PORT_LABELS: Record<IntegrationPort, string> = {
  supabase: '서버 (Supabase)',
  notify: '알림 채널',
  llm: 'LLM',
  push: '앱 푸시',
};

export type BadgeTone = 'live' | 'mock' | 'off' | 'warn';

export interface Badge {
  label: string;
  tone: BadgeTone;
}

const HEALTH_TEXT: Record<string, string> = {
  auth_expired: '로그인이 만료되었습니다. 다시 로그인하세요. (로컬 기능은 그대로 동작합니다)',
  http_error: '서버가 오류 응답을 보냈습니다.',
  timeout: '서버 응답이 없습니다.',
  connect_failed: '서버에 연결하지 못했습니다.',
  host_denied: '허용되지 않은 주소라 연결하지 않았습니다.',
  not_signed_in: '로그인이 필요합니다.',
  secret_store_session_only: '키체인을 쓸 수 없어 키가 이 실행 동안만 유지됩니다. 앱을 다시 시작하면 사라집니다.',
};

/** Mode badge. A degraded port keeps its mode but is flagged, so a live port is never shown as healthy by default. */
export function modeBadge(state: IntegrationState): Badge {
  if (state.mode === 'mock') return { label: '모의 모드', tone: 'mock' };
  if (state.mode === 'notConfigured') return { label: '설정 필요', tone: 'off' };
  if (state.health.kind === 'degraded') return { label: '실제 연결 · 주의', tone: 'warn' };
  return { label: '실제 연결', tone: 'live' };
}

export function healthText(health: Health): string | null {
  if (health.kind !== 'degraded') return null;
  return HEALTH_TEXT[health.code] ?? `문제가 있습니다 (${health.code}).`;
}

/** The keychain fallback is never silent. */
export function secretStoreNotice(status: SecretStoreStatus): string | null {
  if (status.degradedCode === null) return null;
  return HEALTH_TEXT[status.degradedCode] ?? `키 저장소에 문제가 있습니다 (${status.degradedCode}).`;
}

export function authSummary(auth: AuthStatus): string {
  if (auth.degradedCode !== null) return HEALTH_TEXT[auth.degradedCode] ?? `로그인 문제 (${auth.degradedCode}).`;
  return auth.signedIn ? '로그인됨' : '로그인하지 않음';
}

export function verifyMessage(outcome: VerifyOutcome, nowMs: number): string {
  switch (outcome.status) {
    case 'signedIn':
      return '로그인했습니다.';
    case 'rejectedLocked':
      return '계획된 공부 진행 중이라 로그인하지 않았습니다. 시도는 기록에 남았습니다.';
    case 'invalidCode':
      return '코드가 맞지 않습니다.';
    case 'cooldown': {
      const seconds = Math.max(1, Math.ceil((outcome.untilMs - nowMs) / 1000));
      return `잘못된 코드를 여러 번 입력했습니다. ${seconds}초 뒤에 다시 시도하세요.`;
    }
    case 'rateLimited':
      return `요청이 너무 많습니다. ${Math.max(1, Math.ceil(outcome.retryAfterMs / 1000))}초 뒤에 다시 시도하세요.`;
  }
}

/**
 * The key field is emptied after every save attempt, accepted or not, so a typed key never lingers
 * in component state or on screen.
 */
export function draftAfterSave(): string {
  return '';
}

export function saveSecretMessage(outcome: UpdateOutcome): string {
  return outcome.outcome === 'applied'
    ? '저장했습니다.'
    : '계획된 공부 진행 중이라 키를 저장하지 않았습니다. 시도는 기록에 남았습니다.';
}

export function connectionMessage(ok: boolean, code: string): string {
  if (ok) return code === 'mock' ? '모의 모드: 외부로 아무것도 보내지 않았습니다.' : '연결되었습니다.';
  return HEALTH_TEXT[code] ?? `연결하지 못했습니다 (${code}).`;
}

const KIND_LABELS: Record<string, string> = {
  probe: '연결 시험',
  auth_otp: '로그인 코드 요청',
  auth_verify: '로그인 코드 확인',
  auth_refresh: '로그인 갱신',
  auth_logout: '로그아웃',
  device_register: '기기 등록',
  device_remove: '기기 삭제',
  messages: 'LLM 요청 (자료 조각·질문)',
};

export function eventKindLabel(kind: string): string {
  return KIND_LABELS[kind] ?? kind;
}

export function formatClock(ms: number): string {
  return new Date(ms).toLocaleString('ko-KR');
}
