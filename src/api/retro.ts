// Mirror of src-tauri/src/commands/retro.rs: session retrospective merge and wiki lint.

import { invoke } from '@tauri-apps/api/core';
import type { UpdateOutcome } from './settings';

export type DiffKind = 'same' | 'added' | 'removed';

export interface DiffLine {
  kind: DiffKind;
  text: string;
}

export type RetroOutcome =
  | { kind: 'preview'; previewId: string; runId: number; subjectId: number; pageName: string; created: boolean; diff: DiffLine[] }
  | { kind: 'applied'; runId: number; subjectId: number; pageName: string; created: boolean; backedUp: boolean }
  | { kind: 'pending'; runId: number; code: string };

export type RetroStatus = 'pending' | 'merged' | 'skipped' | 'failed' | 'undone';

export interface RetroView {
  runId: number;
  status: RetroStatus;
  subjectId: number | null;
  pageName: string | null;
  attempts: number;
  lastCode: string | null;
  canUndo: boolean;
  createdAtMs: number;
  mergedAtMs: number | null;
}

export type LintKind = 'empty' | 'broken_link' | 'duplicate' | 'orphan';

export interface LintFinding {
  kind: LintKind;
  pageName: string;
  detail: string | null;
}

export interface LintSuggestion {
  pageName: string;
  advice: string;
}

export interface LintReport {
  subjectId: number;
  pages: number;
  findings: LintFinding[];
  suggestions: LintSuggestion[];
  /** Why there is no AI advice; null when it came. */
  adviceCode: string | null;
}

export const retroCommands = {
  submitRetro: (runId: number, subjectId: number, pageName: string | null, note: string) =>
    invoke<RetroOutcome>('submit_retro', { runId, subjectId, pageName, note }),
  retryRetro: (runId: number) => invoke<RetroOutcome>('retry_retro', { runId }),
  applyRetro: (previewId: string) => invoke<RetroOutcome>('apply_retro', { previewId }),
  cancelRetro: (previewId: string) => invoke<void>('cancel_retro', { previewId }),
  undoRetro: (runId: number) => invoke<RetroView>('undo_retro', { runId }),
  skipRetro: (runId: number) => invoke<void>('skip_retro', { runId }),
  getRetro: (runId: number) => invoke<RetroView | null>('get_retro', { runId }),
  listOpenRetros: () => invoke<RetroView[]>('list_open_retros'),
  lintWiki: (subjectId: number) => invoke<LintReport>('lint_wiki', { subjectId }),
  getRetroAutoApply: () => invoke<boolean>('get_retro_auto_apply'),
  setRetroAutoApply: (value: boolean) => invoke<UpdateOutcome>('set_retro_auto_apply', { value }),
};
