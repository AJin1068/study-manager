import { describe, expect, it } from 'vitest';
import {
  isCandidateSet,
  isCharacterView,
  isChooseOutcome,
} from './character';
import characterFixture from './fixtures/character-contract.json';
import shopFixture from './fixtures/shop-contract.json';
import { isPurchaseOutcome, isShopView } from './shop';

// The same fixtures are serialized from Rust in `commands::character::tests` and `commands::shop::tests`.
describe('character contract', () => {
  it('fixture values pass the production guards', () => {
    expect(isCharacterView(characterFixture.view)).toBe(true);
    expect(isCandidateSet(characterFixture.candidateSet)).toBe(true);
    expect(isChooseOutcome(characterFixture.outcome)).toBe(true);
  });

  it('rejects snake_case, unknown stages, and missing slots', () => {
    expect(isCharacterView({ ...characterFixture.view, stage_label: 'x', stageLabel: undefined })).toBe(false);
    expect(isCharacterView({ ...characterFixture.view, stage: 'tree' })).toBe(false);
    expect(isCharacterView({ ...characterFixture.view, equipped: { hat: null, top: null } })).toBe(false);
    expect(isCandidateSet({ ...characterFixture.candidateSet, candidates: [{ name: 'x' }] })).toBe(false);
  });

  it('never carries the setup answers back', () => {
    const text = JSON.stringify(characterFixture);
    for (const key of ['mbti', 'birth', 'gender']) expect(text.toLowerCase()).not.toContain(key);
  });
});

describe('shop contract', () => {
  it('fixture values pass the production guards', () => {
    expect(isShopView(shopFixture.shop)).toBe(true);
    for (const outcome of shopFixture.outcomes) expect(isPurchaseOutcome(outcome)).toBe(true);
  });

  it('rejects unknown kinds, slots, and outcomes', () => {
    expect(isShopView({ ...shopFixture.shop, items: [{ ...shopFixture.shop.items[0], kind: 'pet' }] })).toBe(false);
    expect(isShopView({ ...shopFixture.shop, items: [{ ...shopFixture.shop.items[0], slot: 'feet' }] })).toBe(false);
    expect(isPurchaseOutcome({ outcome: 'insufficient_points', balance: 1 })).toBe(false);
    expect(isPurchaseOutcome({ outcome: 'refunded' })).toBe(false);
  });
});
