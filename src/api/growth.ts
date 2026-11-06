// Mirror of src-tauri/src/commands/growth.rs. Types match the Rust serde output and are checked
// against src/api/fixtures/growth-contract.json by both test suites.

import { invoke } from '@tauri-apps/api/core';
import type { UpdateOutcome } from './settings';

export type ReviewReason = 'empty' | 'low_accuracy' | 'stale' | 'hard' | 'review' | 'chosen';

export interface NextReview {
  subject: string;
  pageName: string;
  reason: ReviewReason;
}

export interface GrowthStatus {
  /** Recognized study time since the first record. */
  totalStudyMs: number;
  level: number;
  stageLabel: string;
  earnedPoints: number;
  /** Total earned points at which the next level starts; null at the top level. */
  nextLevelAt: number | null;
  /** 0-100 inside the current level; 100 at the top. */
  levelProgressPct: number;
  wikiPages: number;
  quizAnswered: number;
  quizCorrect: number;
  /** Null until a question has been answered. */
  quizAccuracyPct: number | null;
  nextReview: NextReview | null;
}

export interface WeeklyInsight {
  weekStart: string;
  text: string;
  /** `rule` when the local rule wrote it; `fallbackReason` says why the model did not. */
  source: 'llm' | 'rule';
  fallbackReason: string | null;
}

const REASONS: readonly string[] = ['empty', 'low_accuracy', 'stale', 'hard', 'review', 'chosen'];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

const numOrNull = (v: unknown): v is number | null => v === null || typeof v === 'number';

export function isNextReview(value: unknown): value is NextReview {
  return isRecord(value) && typeof value.subject === 'string' && typeof value.pageName === 'string' && typeof value.reason === 'string' && REASONS.includes(value.reason);
}

export function isGrowthStatus(value: unknown): value is GrowthStatus {
  return (
    isRecord(value) &&
    typeof value.totalStudyMs === 'number' &&
    typeof value.level === 'number' &&
    typeof value.stageLabel === 'string' &&
    typeof value.earnedPoints === 'number' &&
    numOrNull(value.nextLevelAt) &&
    typeof value.levelProgressPct === 'number' &&
    typeof value.wikiPages === 'number' &&
    typeof value.quizAnswered === 'number' &&
    typeof value.quizCorrect === 'number' &&
    numOrNull(value.quizAccuracyPct) &&
    (value.nextReview === null || isNextReview(value.nextReview))
  );
}

export function isWeeklyInsight(value: unknown): value is WeeklyInsight {
  return (
    isRecord(value) &&
    typeof value.weekStart === 'string' &&
    typeof value.text === 'string' &&
    (value.source === 'llm' || value.source === 'rule') &&
    (value.fallbackReason === null || typeof value.fallbackReason === 'string')
  );
}

export const growthCommands = {
  getGrowthStatus: () => invoke<GrowthStatus>('get_growth_status'),
  /** `refresh` asks the model again even if it already wrote this week's sentence. */
  getWeeklyInsight: (refresh = false) => invoke<WeeklyInsight>('get_weekly_insight', { refresh }),
  /** Whether the weakest subject's name may be part of the insight request (default on). */
  getInsightSubject: () => invoke<boolean>('get_insight_subject'),
  setInsightSubject: (value: boolean) => invoke<UpdateOutcome>('set_insight_subject', { value }),
};
