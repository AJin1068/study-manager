// Mirror of src-tauri/src/commands/release.rs and the `pass_changed` event.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { isQuizQuestion, type QuizQuestion } from './quiz';

export type ReleaseKind = 'emergencyPass' | 'skipToday' | 'earlyRelease';

export type ReleaseProofType = 'typing' | 'quiz';

export interface ReleaseChallenge {
  challengeId: number;
  kind: ReleaseKind;
  /** `quiz` only for an early release with enough wiki questions. */
  proof: ReleaseProofType;
  /** The sentence to type (empty for a quiz). The core compares the typed text itself; the UI never decides. */
  sentence: string;
  /** The questions of a quiz challenge, without answers; empty for typing. */
  questions: QuizQuestion[];
  /** Quiz attempts left today after this one was opened; null for typing. */
  attemptsLeft: number | null;
}

/** What the early-release card knows before it starts. */
export interface ReleaseQuizStatus {
  mode: 'typing' | 'quiz' | 'cooldown' | 'exhausted';
  required: number;
  drawable: number;
  attemptsUsed: number;
  maxAttempts: number;
  cooldownRemainingMs: number;
  cooldownMin: number;
}

export type ReleaseOutcome =
  | { outcome: 'released'; kind: ReleaseKind; expiresAtMs: number | null }
  | { outcome: 'mismatch'; attempts: number }
  | { outcome: 'noChallenge' }
  | { outcome: 'nothingToRelease' }
  | { outcome: 'limitReached'; limit: number }
  | { outcome: 'alreadyActive'; expiresAtMs: number }
  /** Not passed. It never says how many answers were right. */
  | { outcome: 'quizFailed'; retryAfterMs: number; attemptsLeft: number };

export interface PassStatus {
  dailyLimit: number;
  usedToday: number;
  remainingToday: number;
  /** End of the running emergency pass, if any. */
  activeUntilMs: number | null;
  durationMin: number;
  /** Points deducted per emergency pass (applied by the point ledger). */
  penaltyPoints: number;
}

/** One recorded release, newest first from `list_passes`. `kind` uses the stored snake_case names. */
export interface PassRecord {
  id: number;
  atMs: number;
  date: string;
  kind: 'emergency_pass' | 'skip_today' | 'early_release';
  runId: number | null;
  expiresAtMs: number | null;
  challengeLen: number;
  typingMs: number;
  penaltyPoints: number | null;
  proofKind: 'typing' | 'quiz';
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function isNullableNumber(value: unknown): boolean {
  return value === null || typeof value === 'number';
}

export function isReleaseKind(value: unknown): value is ReleaseKind {
  return value === 'emergencyPass' || value === 'skipToday' || value === 'earlyRelease';
}

export function isReleaseChallenge(value: unknown): value is ReleaseChallenge {
  return (
    isRecord(value) &&
    typeof value.challengeId === 'number' &&
    isReleaseKind(value.kind) &&
    (value.proof === 'typing' || value.proof === 'quiz') &&
    typeof value.sentence === 'string' &&
    Array.isArray(value.questions) &&
    value.questions.every(isQuizQuestion) &&
    isNullableNumber(value.attemptsLeft) &&
    value.attemptsLeft !== undefined
  );
}

export function isReleaseOutcome(value: unknown): value is ReleaseOutcome {
  if (!isRecord(value)) return false;
  switch (value.outcome) {
    case 'released':
      return isReleaseKind(value.kind) && isNullableNumber(value.expiresAtMs) && value.expiresAtMs !== undefined;
    case 'mismatch':
      return typeof value.attempts === 'number';
    case 'noChallenge':
    case 'nothingToRelease':
      return true;
    case 'limitReached':
      return typeof value.limit === 'number';
    case 'alreadyActive':
      return typeof value.expiresAtMs === 'number';
    case 'quizFailed':
      return typeof value.retryAfterMs === 'number' && typeof value.attemptsLeft === 'number';
    default:
      return false;
  }
}

export function isReleaseQuizStatus(value: unknown): value is ReleaseQuizStatus {
  return (
    isRecord(value) &&
    (value.mode === 'typing' || value.mode === 'quiz' || value.mode === 'cooldown' || value.mode === 'exhausted') &&
    typeof value.required === 'number' &&
    typeof value.drawable === 'number' &&
    typeof value.attemptsUsed === 'number' &&
    typeof value.maxAttempts === 'number' &&
    typeof value.cooldownRemainingMs === 'number' &&
    typeof value.cooldownMin === 'number'
  );
}

export function isPassStatus(value: unknown): value is PassStatus {
  return (
    isRecord(value) &&
    typeof value.dailyLimit === 'number' &&
    typeof value.usedToday === 'number' &&
    typeof value.remainingToday === 'number' &&
    isNullableNumber(value.activeUntilMs) &&
    value.activeUntilMs !== undefined &&
    typeof value.durationMin === 'number' &&
    typeof value.penaltyPoints === 'number'
  );
}

export function isPassRecord(value: unknown): value is PassRecord {
  return (
    isRecord(value) &&
    typeof value.id === 'number' &&
    typeof value.atMs === 'number' &&
    typeof value.date === 'string' &&
    (value.kind === 'emergency_pass' || value.kind === 'skip_today' || value.kind === 'early_release') &&
    isNullableNumber(value.runId) &&
    value.runId !== undefined &&
    isNullableNumber(value.expiresAtMs) &&
    value.expiresAtMs !== undefined &&
    typeof value.challengeLen === 'number' &&
    typeof value.typingMs === 'number' &&
    isNullableNumber(value.penaltyPoints) &&
    value.penaltyPoints !== undefined &&
    (value.proofKind === 'typing' || value.proofKind === 'quiz')
  );
}

export const releaseCommands = {
  beginRelease: (kind: ReleaseKind) => invoke<ReleaseChallenge>('begin_release', { kind }),
  submitRelease: (challengeId: number, typed: string) =>
    invoke<ReleaseOutcome>('submit_release', { challengeId, typed }),
  /** Early-release quiz only: one answer per question, in order. Graded in the core. */
  submitReleaseQuiz: (challengeId: number, answers: string[]) =>
    invoke<ReleaseOutcome>('submit_release_quiz', { challengeId, answers }),
  getReleaseQuizStatus: () => invoke<ReleaseQuizStatus>('get_release_quiz_status'),
  getPassStatus: () => invoke<PassStatus>('get_pass_status'),
  listPasses: () => invoke<PassRecord[]>('list_passes'),
};

/** Fired after a release is granted, with the new allowance. */
export function onPassChanged(handler: (status: PassStatus) => void): Promise<UnlistenFn> {
  return listen<PassStatus>('pass_changed', (event) => handler(event.payload));
}
