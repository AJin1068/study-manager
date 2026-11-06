// Mirror of src-tauri/src/commands/quiz.rs and the `quiz://card` event. A question never carries
// its answer; the core grades what the UI submits.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { UpdateOutcome } from './settings';

export type QuizSettingKey =
  | 'releaseCooldownMin'
  | 'releaseMaxAttemptsPerDay'
  | 'distractionEnabled'
  | 'checkinEnabled'
  | 'sessionEndEnabled';

export interface QuizConfig {
  releaseCooldownMin: number;
  releaseMaxAttemptsPerDay: number;
  distractionEnabled: boolean;
  checkinEnabled: boolean;
  sessionEndEnabled: boolean;
}

export type QuizItemKind = 'mcq' | 'cloze';
export type QuizContext = 'distraction' | 'release' | 'session_end' | 'checkin' | 'review';
export type QuizReportReason = 'wrong_answer' | 'unclear' | 'off_topic' | 'other';

export interface QuizQuestion {
  itemId: number;
  kind: QuizItemKind;
  prompt: string;
  /** Empty for a fill-in-the-blank question. */
  choices: string[];
}

export interface QuizCard {
  cardId: number;
  context: QuizContext;
  questions: QuizQuestion[];
  /** Only the session-end quiz may be skipped (the skip is recorded). */
  skippable: boolean;
}

export interface QuizItemResult {
  itemId: number;
  correct: boolean;
  answer: string;
}

export interface QuizCardResult {
  cardId: number;
  context: QuizContext;
  results: QuizItemResult[];
  skipped: boolean;
  bonusPoints: number;
}

export interface QuizBankStats {
  drawable: number;
  reported: number;
}

export interface QuizGenerateReport {
  pages: number;
  added: number;
  dropped: number;
  fromModel: number;
  fromTemplate: number;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((v) => typeof v === 'string');
}

const CONTEXTS: readonly string[] = ['distraction', 'release', 'session_end', 'checkin', 'review'];

export function isQuizQuestion(value: unknown): value is QuizQuestion {
  return (
    isRecord(value) &&
    typeof value.itemId === 'number' &&
    (value.kind === 'mcq' || value.kind === 'cloze') &&
    typeof value.prompt === 'string' &&
    isStringArray(value.choices)
  );
}

export function isQuizCard(value: unknown): value is QuizCard {
  return (
    isRecord(value) &&
    typeof value.cardId === 'number' &&
    typeof value.context === 'string' &&
    CONTEXTS.includes(value.context) &&
    Array.isArray(value.questions) &&
    value.questions.every(isQuizQuestion) &&
    typeof value.skippable === 'boolean'
  );
}

export function isQuizCardResult(value: unknown): value is QuizCardResult {
  return (
    isRecord(value) &&
    typeof value.cardId === 'number' &&
    typeof value.context === 'string' &&
    CONTEXTS.includes(value.context) &&
    Array.isArray(value.results) &&
    value.results.every(
      (r: unknown) =>
        isRecord(r) && typeof r.itemId === 'number' && typeof r.correct === 'boolean' && typeof r.answer === 'string',
    ) &&
    typeof value.skipped === 'boolean' &&
    typeof value.bonusPoints === 'number'
  );
}

export function isQuizConfig(value: unknown): value is QuizConfig {
  return (
    isRecord(value) &&
    typeof value.releaseCooldownMin === 'number' &&
    typeof value.releaseMaxAttemptsPerDay === 'number' &&
    typeof value.distractionEnabled === 'boolean' &&
    typeof value.checkinEnabled === 'boolean' &&
    typeof value.sessionEndEnabled === 'boolean'
  );
}

export const quizCommands = {
  getQuizSettings: () => invoke<QuizConfig>('get_quiz_settings'),
  updateQuizSetting: (key: QuizSettingKey, value: number | boolean) =>
    invoke<UpdateOutcome>('update_quiz_setting', { key, value }),
  getQuizStats: () => invoke<QuizBankStats>('get_quiz_stats'),
  generateQuizBank: (subjectId: number) => invoke<QuizGenerateReport>('generate_quiz_bank', { subjectId }),
  flagQuizItem: (itemId: number, reason: QuizReportReason) => invoke<void>('flag_quiz_item', { itemId, reason }),
  startReviewQuiz: (subjectId: number | null, count: number) =>
    invoke<QuizCard | null>('start_review_quiz', { subjectId, count }),
  getSessionEndQuiz: () => invoke<QuizCard | null>('get_session_end_quiz'),
  getCheckinQuiz: () => invoke<QuizCard | null>('get_checkin_quiz'),
  /** `answers` has one entry per question in order; `skip` skips the session-end quiz or dismisses a card. */
  answerQuizCard: (cardId: number, answers: string[], skip: boolean) =>
    invoke<QuizCardResult | null>('answer_quiz_card', { cardId, answers, skip }),
};

/** A review question for the window (blocked app or site opened). It never lifts a block. */
export function onQuizCard(handler: (card: QuizCard) => void): Promise<UnlistenFn> {
  return listen<QuizCard>('quiz://card', (event) => handler(event.payload));
}
