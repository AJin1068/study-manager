// Display helpers for app blocking. The Rust core decides everything; these only label it.

const ACTION_LABEL: Record<string, string> = {
  would_terminate: '종료했을 것(시험 모드)',
  terminate_requested: '정상 종료 요청',
  terminated: '종료됨',
  force_killed: '강제 종료',
  failed: '실패',
  skipped_protected: '보호 앱이라 건너뜀',
  skipped_pass: '패스로 건너뜀',
};

export function actionLabel(action: string): string {
  return ACTION_LABEL[action] ?? action;
}

const DETAIL_LABEL: Record<string, string> = {
  unsupported: '이 OS에서 지원하지 않음',
  os_refused: 'OS가 거절함',
  grace_expired_force_not_allowed: '유예 시간이 지났지만 강제 종료가 허용되지 않음',
};

export function detailLabel(detail: string | null): string {
  return detail === null ? '' : (DETAIL_LABEL[detail] ?? detail);
}

/** Valid grace range, mirrored from the Rust setting spec (1..=60 seconds). */
export const GRACE_MIN = 1;
export const GRACE_MAX = 60;

export function parseGrace(raw: string): { ok: true; value: number } | { ok: false; error: string } {
  const text = raw.trim();
  if (!/^\d+$/.test(text)) return { ok: false, error: `${GRACE_MIN}~${GRACE_MAX} 사이 정수를 입력하세요.` };
  const value = Number(text);
  if (value < GRACE_MIN || value > GRACE_MAX) {
    return { ok: false, error: `${GRACE_MIN}~${GRACE_MAX}초 범위여야 합니다.` };
  }
  return { ok: true, value };
}
