import type { BlockerStatus } from '../api';

export const ANDROID_INSTALL_STEPS: readonly string[] = [
  '폰의 카메라나 QR 앱으로 아래 QR을 스캔해 APK를 내려받습니다.',
  '설정에서 "출처를 알 수 없는 앱 설치"를 이 브라우저에 한 번 허용합니다.',
  '내려받은 APK를 열어 설치한 뒤, 앱의 안내에 따라 PC와 짝을 맺습니다.',
];

export const SIDELOAD_NOTE =
  '이 앱은 스토어 밖에서 직접 설치합니다. 링크가 내 저장소의 Releases가 맞는지 확인하고, 모르는 출처의 APK는 설치하지 마세요.';

export const IOS_ALTERNATIVES: readonly string[] = [
  '아이폰의 설정 → 스크린 타임에서 앱 제한이나 휴식 시간을 직접 설정하세요.',
  '집중 모드(방해 금지)를 공부 시간에 맞춰 예약하세요.',
  '공부하는 동안 폰을 다른 방에 두거나 상자에 넣어 두세요.',
];

/** Text for the iPhone row. Only the Screen Time reason exists; anything else is shown by code. */
export function iosStatusText(status: BlockerStatus): string {
  if (status.kind === 'unsupported') {
    return 'iPhone은 지원하지 않습니다. 애플의 스크린 타임은 기기 사용자가 직접 승인해야 하고 앱이 대신 잠글 수 없습니다.';
  }
  return status.kind === 'active' ? 'iPhone 잠금 사용 중' : 'iPhone 잠금 대기';
}
