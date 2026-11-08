// Display and form helpers for the character setup and the shop. The core decides everything
// (candidates, prices, ownership); these functions only shape input and wording.

import type { CandidateSet, Gender, SetupInput, ShopItemView, PurchaseOutcome, SlotName } from '../api';

export const MBTI_TYPES = [
  'INTJ', 'INTP', 'ENTJ', 'ENTP',
  'INFJ', 'INFP', 'ENFJ', 'ENFP',
  'ISTJ', 'ISFJ', 'ESTJ', 'ESFJ',
  'ISTP', 'ISFP', 'ESTP', 'ESFP',
] as const;

export const GENDER_LABEL: Record<Gender, string> = { female: '여성', male: '남성', other: '기타' };

export interface SetupForm {
  mbti: string;
  birthYear: string;
  gender: string;
}

export const EMPTY_FORM: SetupForm = { mbti: '', birthYear: '', gender: '' };

export type FormResult = { ok: true; input: SetupInput } | { ok: false; message: string };

/** A blank field is a skipped field. A birth year must be a whole year in range. */
export function setupInputFromForm(form: SetupForm, currentYear: number): FormResult {
  const input: SetupInput = {};
  if (form.mbti !== '') {
    if (!(MBTI_TYPES as readonly string[]).includes(form.mbti)) return { ok: false, message: 'MBTI를 목록에서 골라 주세요.' };
    input.mbti = form.mbti;
  }
  const year = form.birthYear.trim();
  if (year !== '') {
    const parsed = Number(year);
    if (!/^\d{4}$/.test(year) || parsed < 1900 || parsed > currentYear) {
      return { ok: false, message: `태어난 해는 1900~${currentYear} 사이의 4자리 숫자로 적어 주세요.` };
    }
    input.birthYear = parsed;
  }
  if (form.gender !== '') {
    if (!Object.keys(GENDER_LABEL).includes(form.gender)) return { ok: false, message: '성별을 목록에서 골라 주세요.' };
    input.gender = form.gender as Gender;
  }
  return { ok: true, input };
}

/** The next seed of "다시 뽑기". */
export function nextSeed(set: Pick<CandidateSet, 'seed'> | null): number {
  return set === null ? 0 : (set.seed + 1) % 1_000_000;
}

/** Why the names are built-in, in words; null when the model named them. */
export function nameSourceNote(set: Pick<CandidateSet, 'fallbackReason'>): string | null {
  if (set.fallbackReason === null) return null;
  return set.fallbackReason === 'bad_answer'
    ? '모델 답변을 쓸 수 없는 이름은 기본 이름으로 바꿨어요.'
    : '모델 없이 기본 이름으로 지었어요.';
}

export function svgDataUri(svg: string): string {
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

export type ItemAction = 'buy' | 'equip' | 'unequip' | 'locked' | 'owned';

/** What the item's button does. Achievement items can only be earned; furniture is only owned. */
export function itemAction(item: ShopItemView): ItemAction {
  if (item.owned) {
    if (item.kind === 'furniture') return 'owned';
    if (item.kind === 'character') return item.equipped ? 'owned' : 'equip';
    return item.equipped ? 'unequip' : 'equip';
  }
  return item.source === 'shop' ? 'buy' : 'locked';
}

export function itemPriceText(item: ShopItemView): string {
  if (item.source === 'achievement') return item.achievement ?? '업적';
  if (item.source === 'starter') return '기본';
  return `${item.price}P`;
}

export function purchaseNotice(outcome: PurchaseOutcome): string {
  switch (outcome.outcome) {
    case 'purchased':
      return `샀어요. 남은 포인트 ${outcome.balance}P`;
    case 'already_owned':
      return '이미 가지고 있어요.';
    case 'insufficient_points':
      return `포인트가 ${outcome.price - outcome.balance}P 모자라요. (${outcome.balance}P / ${outcome.price}P)`;
    case 'not_for_sale':
      return '살 수 없는 물건이에요.';
  }
}

export interface ShopGroup {
  key: string;
  label: string;
  items: ShopItemView[];
}

const SLOT_ORDER: SlotName[] = ['hat', 'top', 'accessory', 'hand'];
const SLOT_TITLE: Record<SlotName, string> = { hat: '모자', top: '옷', accessory: '소품', hand: '손에 드는 것' };

/** Clothes by slot, then furniture, then characters; empty groups are dropped. */
export function groupItems(items: ShopItemView[]): ShopGroup[] {
  const groups: ShopGroup[] = SLOT_ORDER.map((slot) => ({
    key: slot,
    label: SLOT_TITLE[slot],
    items: items.filter((i) => i.kind === 'clothing' && i.slot === slot),
  }));
  groups.push({ key: 'furniture', label: '오두막 가구', items: items.filter((i) => i.kind === 'furniture') });
  groups.push({ key: 'character', label: '캐릭터 친구', items: items.filter((i) => i.kind === 'character') });
  return groups.filter((g) => g.items.length > 0);
}

const POINT_REASON: Record<string, string> = {
  focus_window: '30분 집중 달성',
  start_bonus: '계획 시간에 바로 시작',
  emergency_pass: '긴급 패스 사용',
  run_aborted: '계획 중간에 그만둠',
  quiz_bonus: '위키 퀴즈 정답',
  shop_purchase: '상점 구매',
  gift_sent: '선물 보냄',
  gift_refund: '선물 환불',
};

/** Korean label for a point-ledger reason code; unknown codes show as stored. */
export function pointReasonLabel(code: string): string {
  return POINT_REASON[code] ?? code;
}
