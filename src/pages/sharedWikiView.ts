import type { RoomPageView, SharedMerge } from '../api';

const CODE_TEXT: Record<string, string> = {
  ok: '완료했어요.',
  not_confirmed: '확인 화면에서 확인을 눌러야 공유돼요.',
  not_signed_in: '로그인이 필요해요.',
  not_member: '이 방의 멤버가 아니에요.',
  invalid_input: '이 페이지는 공유할 수 없어요.',
  too_large: '페이지가 너무 커서 공유할 수 없어요.',
  too_many: '이 방에 올릴 수 있는 페이지 수를 넘었어요.',
  not_found: '공유된 페이지를 찾지 못했어요.',
  not_shared: '공유한 적이 없는 페이지예요.',
  not_consented: '합치기에 동의하지 않은 글이 들어 있어요.',
  merge_not_enough: '합치기를 허용한 글이 둘 이상 있어야 합칠 수 있어요.',
};

/** Words for a server or app code; never the raw code. */
export function sharedCodeText(code: string): string {
  return CODE_TEXT[code] ?? '처리하지 못했어요. 잠시 뒤에 다시 해 주세요.';
}

export const WITHDRAW_LIMIT = '철회해도 이미 본 사람의 사본은 회수할 수 없어요.';

export function pageSummary(page: RoomPageView): string {
  const parts = [page.contributors.length > 0 ? `기여: ${page.contributors.join(', ')}` : '기여자 없음'];
  if (page.hasMerge) parts.push('합본 있음');
  if (page.branched) parts.push('동시에 고친 글이 나란히 있어요');
  if (page.listedOnly.length > 0) parts.push(`합치기 미동의(목록만): ${page.listedOnly.join(', ')}`);
  return parts.join(' · ');
}

export function canMerge(page: RoomPageView): boolean {
  return page.mergeableParts >= 2;
}

export function mergeSummary(merge: SharedMerge): string {
  if (merge.code !== 'ok') return sharedCodeText(merge.code);
  const who = merge.contributors.join(', ');
  const how = merge.usedLlm ? 'AI가 합쳤어요' : 'AI 없이 사람별로 이어 붙였어요';
  return `${how} (기여: ${who}).`;
}
