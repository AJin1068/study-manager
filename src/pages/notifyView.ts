import type { SyncStatus } from '../api';
import { formatClock } from './integrationView';

export const NOTIFY_CHANNELS = [
  { value: 'none', label: '사용 안 함' },
  { value: 'ntfy', label: 'ntfy (앱 설치, 계정 없음)' },
  { value: 'telegram', label: 'Telegram' },
] as const;

export const MIN_GRACE_MIN = 1;
export const MAX_GRACE_MIN = 3;

export const TOPIC_WARNING =
  '이 토픽은 비밀번호와 같습니다. 아는 사람은 누구나 이 알림을 볼 수 있으니 남에게 보여 주지 마세요. 알림 문구에는 과목·계획 이름이 들어가지 않고 시각만 나옵니다.';

export type GraceParse = { ok: true; value: number } | { ok: false; message: string };

/** Same bound as the core (1..3): detection plus delivery must stay under 5 minutes. */
export function parseGrace(text: string): GraceParse {
  const trimmed = text.trim();
  if (!/^\d+$/.test(trimmed)) return { ok: false, message: '1에서 3 사이의 정수로 입력하세요.' };
  const value = Number(trimmed);
  if (value < MIN_GRACE_MIN || value > MAX_GRACE_MIN) {
    return { ok: false, message: '유예는 1~3분입니다. 더 길면 5분 안에 알리겠다는 약속을 지킬 수 없습니다.' };
  }
  return { ok: true, value };
}

export type SignalTone = 'off' | 'ok' | 'warn';

export interface SignalSummary {
  tone: SignalTone;
  headline: string;
  details: string[];
}

const ERROR_TEXT: Record<string, string> = {
  timeout: '서버 응답이 없습니다.',
  connect_failed: '서버에 연결하지 못했습니다.',
  http_error: '서버가 오류 응답을 보냈습니다.',
  host_denied: '허용되지 않은 주소라 보내지 않았습니다.',
  device_not_owned: '이 기기가 서버에서 지워져 다시 등록합니다.',
  auth_expired: '로그인이 만료되었습니다.',
  not_signed_in: '로그인이 필요합니다.',
};

function errorText(code: string): string {
  return ERROR_TEXT[code] ?? `문제가 있습니다 (${code}).`;
}

/** Three states: off (nothing leaves the PC), working, and needs attention. */
export function signalSummary(status: SyncStatus): SignalSummary {
  if (!status.active) {
    return { tone: 'off', headline: '꺼짐: 로그인하고 서버를 설정하기 전에는 아무것도 보내지 않습니다.', details: [] };
  }
  const details = [
    status.lastBeatOkMs !== null ? `마지막 신호 ${formatClock(status.lastBeatOkMs)}` : '아직 신호를 보내지 못했습니다.',
    status.lastSyncOkMs !== null ? `마지막 동기화 ${formatClock(status.lastSyncOkMs)}` : '아직 동기화하지 못했습니다.',
  ];
  const problems: string[] = [];
  if (status.degradedCode !== null) problems.push('서버가 PC의 신호를 받지 못하고 있습니다. 이 상태에서는 놓친 계획 알림이 늦거나 잘못 갈 수 있습니다.');
  if (status.beatError !== null) problems.push(`신호: ${errorText(status.beatError)}`);
  if (status.syncError !== null) problems.push(`동기화: ${errorText(status.syncError)}`);
  if (problems.length > 0) return { tone: 'warn', headline: '주의', details: [...details, ...problems] };
  return { tone: 'ok', headline: '정상', details };
}

export function testResultMessage(ok: boolean, code: string): string {
  if (ok) return code === 'ok' ? '테스트 알림을 보냈습니다. 폰에 도착했는지 확인하세요.' : code;
  const text: Record<string, string> = {
    not_configured: '채널 설정이 끝나지 않았습니다.',
    unsafe_server: 'ntfy 서버 주소가 안전하지 않아 보내지 않았습니다.',
    telegram_not_configured: '서버에 Telegram 봇이 설정되지 않았습니다.',
  };
  return text[code] ?? ERROR_TEXT[code] ?? `보내지 못했습니다 (${code}).`;
}

/** The QR is an SVG string from the core; it is shown as an image, never injected as markup. */
export function svgDataUri(svg: string): string {
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}
