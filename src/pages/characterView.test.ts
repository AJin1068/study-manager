import { describe, expect, it } from 'vitest';
import type { ShopItemView } from '../api';
import {
  EMPTY_FORM,
  MBTI_TYPES,
  groupItems,
  itemAction,
  itemPriceText,
  nameSourceNote,
  nextSeed,
  purchaseNotice,
  setupInputFromForm,
  svgDataUri,
} from './characterView';

function item(over: Partial<ShopItemView>): ShopItemView {
  return {
    id: 'hat_beanie',
    kind: 'clothing',
    slot: 'hat',
    label: '비니',
    price: 40,
    source: 'shop',
    giftable: true,
    achievement: null,
    owned: false,
    equipped: false,
    affordable: true,
    ...over,
  };
}

describe('setup form', () => {
  it('every field can be skipped', () => {
    expect(setupInputFromForm(EMPTY_FORM, 2027)).toEqual({ ok: true, input: {} });
    expect(setupInputFromForm({ mbti: 'INFP', birthYear: '', gender: '' }, 2027)).toEqual({ ok: true, input: { mbti: 'INFP' } });
    expect(setupInputFromForm({ mbti: '', birthYear: ' 1999 ', gender: 'female' }, 2027)).toEqual({
      ok: true,
      input: { birthYear: 1999, gender: 'female' },
    });
  });

  it('offers the 16 distinct types', () => {
    expect(new Set(MBTI_TYPES).size).toBe(16);
  });

  it('rejects out-of-range or malformed answers at the boundaries', () => {
    for (const year of ['1899', '2028', '99', '19999', 'abcd', '19.9', '-199']) {
      expect(setupInputFromForm({ ...EMPTY_FORM, birthYear: year }, 2027).ok, year).toBe(false);
    }
    expect(setupInputFromForm({ ...EMPTY_FORM, birthYear: '1900' }, 2027).ok).toBe(true);
    expect(setupInputFromForm({ ...EMPTY_FORM, birthYear: '2027' }, 2027).ok).toBe(true);
    expect(setupInputFromForm({ ...EMPTY_FORM, mbti: 'ABCD' }, 2027).ok).toBe(false);
    expect(setupInputFromForm({ ...EMPTY_FORM, gender: 'robot' }, 2027).ok).toBe(false);
  });
});

describe('candidates', () => {
  it('re-roll counts the seed up and wraps', () => {
    expect(nextSeed(null)).toBe(0);
    expect(nextSeed({ seed: 4 })).toBe(5);
    expect(nextSeed({ seed: 999_999 })).toBe(0);
  });

  it('explains built-in names', () => {
    expect(nameSourceNote({ fallbackReason: null })).toBeNull();
    expect(nameSourceNote({ fallbackReason: 'llm_disabled' })).toContain('기본 이름');
    expect(nameSourceNote({ fallbackReason: 'bad_answer' })).toContain('답변');
  });

  it('turns an svg into an image source', () => {
    const uri = svgDataUri("<svg viewBox='0 0 1 1'></svg>");
    expect(uri.startsWith('data:image/svg+xml;charset=utf-8,')).toBe(true);
    expect(uri).not.toContain('<');
  });
});

describe('shop items', () => {
  it('picks the button from ownership, source, and kind', () => {
    expect(itemAction(item({}))).toBe('buy');
    expect(itemAction(item({ owned: true }))).toBe('equip');
    expect(itemAction(item({ owned: true, equipped: true }))).toBe('unequip');
    expect(itemAction(item({ source: 'achievement', price: 0, achievement: '7일 연속 달성' }))).toBe('locked');
    expect(itemAction(item({ kind: 'furniture', slot: null, owned: true }))).toBe('owned');
    expect(itemAction(item({ kind: 'character', slot: null, owned: true }))).toBe('equip');
    expect(itemAction(item({ kind: 'character', slot: null, owned: true, equipped: true }))).toBe('owned');
  });

  it('prices read by source', () => {
    expect(itemPriceText(item({}))).toBe('40P');
    expect(itemPriceText(item({ source: 'achievement', achievement: '퀴즈 100개 정답' }))).toBe('퀴즈 100개 정답');
    expect(itemPriceText(item({ source: 'starter', price: 0 }))).toBe('기본');
  });

  it('says what happened to a purchase', () => {
    expect(purchaseNotice({ outcome: 'purchased', balance: 60 })).toContain('60P');
    expect(purchaseNotice({ outcome: 'insufficient_points', balance: 39, price: 40 })).toContain('1P 모자라요');
    expect(purchaseNotice({ outcome: 'already_owned' })).toContain('이미');
    expect(purchaseNotice({ outcome: 'not_for_sale' })).toContain('살 수 없는');
  });

  it('groups clothes by slot, then furniture and characters, dropping empty groups', () => {
    const groups = groupItems([
      item({ id: 'a', kind: 'furniture', slot: null }),
      item({ id: 'b', slot: 'top' }),
      item({ id: 'c', slot: 'hat' }),
      item({ id: 'd', kind: 'character', slot: null }),
    ]);
    expect(groups.map((g) => g.key)).toEqual(['hat', 'top', 'furniture', 'character']);
  });
});
