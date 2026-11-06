// Mirror of src-tauri/src/commands/shop.rs. Checked against src/api/fixtures/shop-contract.json.

import { invoke } from '@tauri-apps/api/core';
import { SLOTS, type CharacterView, type SlotName } from './character';

export type ItemKind = 'clothing' | 'furniture' | 'character';
export type ItemSource = 'starter' | 'shop' | 'achievement';

export interface ShopItemView {
  id: string;
  kind: ItemKind;
  slot: SlotName | null;
  label: string;
  price: number;
  source: ItemSource;
  /** Only shop items can be given away. */
  giftable: boolean;
  /** What earns an achievement item. */
  achievement: string | null;
  owned: boolean;
  equipped: boolean;
  affordable: boolean;
}

export interface ShopView {
  balance: number;
  items: ShopItemView[];
}

export type PurchaseOutcome =
  | { outcome: 'purchased'; balance: number }
  | { outcome: 'already_owned' }
  | { outcome: 'insufficient_points'; balance: number; price: number }
  | { outcome: 'not_for_sale' };

const KINDS: readonly string[] = ['clothing', 'furniture', 'character'];
const SOURCES: readonly string[] = ['starter', 'shop', 'achievement'];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

export function isShopItemView(value: unknown): value is ShopItemView {
  return (
    isRecord(value) &&
    typeof value.id === 'string' &&
    typeof value.kind === 'string' &&
    KINDS.includes(value.kind) &&
    (value.slot === null || (typeof value.slot === 'string' && (SLOTS as readonly string[]).includes(value.slot))) &&
    typeof value.label === 'string' &&
    typeof value.price === 'number' &&
    typeof value.source === 'string' &&
    SOURCES.includes(value.source) &&
    typeof value.giftable === 'boolean' &&
    (value.achievement === null || typeof value.achievement === 'string') &&
    typeof value.owned === 'boolean' &&
    typeof value.equipped === 'boolean' &&
    typeof value.affordable === 'boolean'
  );
}

export function isShopView(value: unknown): value is ShopView {
  return isRecord(value) && typeof value.balance === 'number' && Array.isArray(value.items) && value.items.every(isShopItemView);
}

export function isPurchaseOutcome(value: unknown): value is PurchaseOutcome {
  if (!isRecord(value)) return false;
  switch (value.outcome) {
    case 'purchased':
      return typeof value.balance === 'number';
    case 'insufficient_points':
      return typeof value.balance === 'number' && typeof value.price === 'number';
    case 'already_owned':
    case 'not_for_sale':
      return true;
    default:
      return false;
  }
}

export const shopCommands = {
  /** Grants achievement items the records show, then lists the catalog. */
  getShop: () => invoke<ShopView>('get_shop'),
  purchaseItem: (itemId: string) => invoke<PurchaseOutcome>('purchase_item', { itemId }),
  equipItem: (itemId: string) => invoke<CharacterView>('equip_item', { itemId }),
  unequipSlot: (slot: SlotName) => invoke<CharacterView>('unequip_slot', { slot }),
};
