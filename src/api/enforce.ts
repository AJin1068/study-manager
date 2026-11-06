// Mirror of src-tauri/src/commands/enforce.rs and the `block_attempt` event (src-tauri/src/enforce/).

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { UpdateOutcome } from './settings';

export type EnforcementMode = 'dryRun' | 'enforce';

export interface BlockEntry {
  id: number;
  kind: string;
  /** Bundle id (macOS) or normalized executable path (Windows). */
  pattern: string;
  source: string;
  category: string | null;
  enabled: boolean;
  createdAtMs: number;
}

/** Window titles and URLs are never part of an event: `targetId` is an app id. */
export interface BlockEvent {
  id: number;
  atMs: number;
  runId: number | null;
  segmentId: number | null;
  targetKind: string;
  targetId: string;
  action: string;
  mode: string;
  detailCode: string | null;
}

export interface EnforcementStatus {
  mode: EnforcementMode;
  graceWaitSec: number;
  forceQuitAllowed: boolean;
  /** Blocking is in force right now (lock held during a study block). */
  active: boolean;
  /** The sentence that must be typed to switch to `enforce`. */
  enforceConfirmation: string;
  recentEvents: BlockEvent[];
}

export interface RunningApp {
  id: string;
  displayName: string;
  isProtected: boolean;
}

/** Payload of the `block_attempt` event. */
export interface BlockAttempt {
  targetId: string;
  action: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function nullableNumber(value: unknown): boolean {
  return value === null || typeof value === 'number';
}

export function isBlockEntry(value: unknown): value is BlockEntry {
  if (!isRecord(value)) return false;
  return (
    typeof value.id === 'number' &&
    typeof value.kind === 'string' &&
    typeof value.pattern === 'string' &&
    typeof value.source === 'string' &&
    (value.category === null || typeof value.category === 'string') &&
    typeof value.enabled === 'boolean' &&
    typeof value.createdAtMs === 'number'
  );
}

export function isBlockEvent(value: unknown): value is BlockEvent {
  if (!isRecord(value)) return false;
  return (
    typeof value.id === 'number' &&
    typeof value.atMs === 'number' &&
    nullableNumber(value.runId) &&
    nullableNumber(value.segmentId) &&
    typeof value.targetKind === 'string' &&
    typeof value.targetId === 'string' &&
    typeof value.action === 'string' &&
    typeof value.mode === 'string' &&
    (value.detailCode === null || typeof value.detailCode === 'string')
  );
}

export function isEnforcementStatus(value: unknown): value is EnforcementStatus {
  if (!isRecord(value)) return false;
  return (
    (value.mode === 'dryRun' || value.mode === 'enforce') &&
    typeof value.graceWaitSec === 'number' &&
    typeof value.forceQuitAllowed === 'boolean' &&
    typeof value.active === 'boolean' &&
    typeof value.enforceConfirmation === 'string' &&
    Array.isArray(value.recentEvents) &&
    value.recentEvents.every(isBlockEvent)
  );
}

export function isRunningApp(value: unknown): value is RunningApp {
  if (!isRecord(value)) return false;
  return (
    typeof value.id === 'string' &&
    typeof value.displayName === 'string' &&
    typeof value.isProtected === 'boolean'
  );
}

export function isBlockAttempt(value: unknown): value is BlockAttempt {
  if (!isRecord(value)) return false;
  return typeof value.targetId === 'string' && typeof value.action === 'string';
}

export const enforceCommands = {
  listBlockEntries: () => invoke<BlockEntry[]>('list_block_entries'),
  addBlockEntry: (pattern: string, category?: string) =>
    invoke<UpdateOutcome>('add_block_entry', { pattern, category: category ?? null }),
  removeBlockEntry: (id: number) => invoke<UpdateOutcome>('remove_block_entry', { id }),
  getEnforcementStatus: () => invoke<EnforcementStatus>('get_enforcement_status'),
  listRunningApps: () => invoke<RunningApp[]>('list_running_apps'),
  /** `confirmation` must equal `EnforcementStatus.enforceConfirmation` to switch to `enforce`. */
  setEnforcementMode: (mode: EnforcementMode, confirmation: string) =>
    invoke<UpdateOutcome>('set_enforcement_mode', { mode, confirmation }),
};

/** Fired for every block action (would_terminate, terminate_requested, terminated, ...). */
export function onBlockAttempt(handler: (attempt: BlockAttempt) => void): Promise<UnlistenFn> {
  return listen<BlockAttempt>('block_attempt', (event) => handler(event.payload));
}
