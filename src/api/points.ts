// Mirror of src-tauri/src/commands/points.rs. Types must match the Rust serde output.

import { invoke } from '@tauri-apps/api/core';

/** Growth stages in order: 씨앗, 새싹, 작은 나무, 숲의 정령. */
export const GROWTH_STAGES = ['seed', 'sprout', 'small_tree', 'forest_spirit'] as const;
export type GrowthStage = (typeof GROWTH_STAGES)[number];

export interface PointsStatus {
  /** Sum of the ledger. */
  balance: number;
  /** Points earned in total (positive rows only): the level is computed from this, not the balance. */
  earned: number;
  level: number;
  stage: GrowthStage;
  stageLabel: string;
  /** Total earned points at which the next level starts; null at the top level. */
  nextLevelAt: number | null;
}

export interface LedgerEntry {
  id: number;
  atMs: number;
  /** Negative for a deduction. */
  delta: number;
  reasonCode: string;
  refKey: string;
}

const STAGE_NAMES: readonly string[] = GROWTH_STAGES;

export function isPointsStatus(value: unknown): value is PointsStatus {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    typeof r.balance === 'number' &&
    typeof r.earned === 'number' &&
    typeof r.level === 'number' &&
    typeof r.stage === 'string' &&
    STAGE_NAMES.includes(r.stage) &&
    typeof r.stageLabel === 'string' &&
    (r.nextLevelAt === null || typeof r.nextLevelAt === 'number')
  );
}

export function isLedgerEntry(value: unknown): value is LedgerEntry {
  if (typeof value !== 'object' || value === null) return false;
  const r = value as Record<string, unknown>;
  return (
    typeof r.id === 'number' &&
    typeof r.atMs === 'number' &&
    typeof r.delta === 'number' &&
    typeof r.reasonCode === 'string' &&
    typeof r.refKey === 'string'
  );
}

export const pointsCommands = {
  getPointsStatus: () => invoke<PointsStatus>('get_points_status'),
  listPointLedger: (limit: number) => invoke<LedgerEntry[]>('list_point_ledger', { limit }),
};
