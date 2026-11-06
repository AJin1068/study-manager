// Mirror of src-tauri/src/commands/character.rs and shop.rs. Types match the Rust serde output and
// are checked against src/api/fixtures/character-contract.json and shop-contract.json by both
// test suites. MBTI, birth year, and gender go to the core only as setup input and never come back.

import { invoke } from '@tauri-apps/api/core';
import { GROWTH_STAGES, type GrowthStage } from './points';

export type Gender = 'male' | 'female' | 'other';

/** The setup answers. Every field may be left out (skipped). */
export interface SetupInput {
  mbti?: string | null;
  birthYear?: number | null;
  gender?: Gender | null;
}

export interface Look {
  species: string;
  palette: string;
  expression: string;
  prop: string;
}

export interface LookLabels {
  species: string;
  palette: string;
  expression: string;
  prop: string;
}

export type SlotName = 'hat' | 'top' | 'accessory' | 'hand';
export const SLOTS: readonly SlotName[] = ['hat', 'top', 'accessory', 'hand'];
export const SLOT_LABEL: Record<SlotName, string> = { hat: '모자', top: '옷', accessory: '소품', hand: '손' };

export interface Equipped {
  hat: string | null;
  top: string | null;
  accessory: string | null;
  hand: string | null;
}

export interface CandidateView {
  look: Look;
  labels: LookLabels;
  starterTop: string;
  name: string;
  blurb: string;
  nameSource: 'llm' | 'builtin';
  svg: string;
}

export interface CandidateSet {
  seed: number;
  candidates: CandidateView[];
  /** Why the names are built-in (a stable code or `bad_answer`); null when the model named all. */
  fallbackReason: string | null;
}

export interface CharacterView {
  name: string;
  blurb: string;
  look: Look;
  labels: LookLabels;
  stage: GrowthStage;
  stageLabel: string;
  level: number;
  equipped: Equipped;
  svg: string;
  chosenAtMs: number;
}

export interface ChooseRequest {
  look: Look;
  starterTop: string;
  name: string;
  blurb: string;
  input: SetupInput;
}

export interface ChooseOutcome {
  character: CharacterView;
  /** The tone the MBTI set as the starting value; null when it set none. */
  toneApplied: string | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

const isStr = (v: unknown): v is string => typeof v === 'string';
const strOrNull = (v: unknown): v is string | null => v === null || typeof v === 'string';

function hasStrings(value: unknown, keys: readonly string[]): boolean {
  return isRecord(value) && keys.every((k) => isStr(value[k]));
}

export function isLook(value: unknown): value is Look {
  return hasStrings(value, ['species', 'palette', 'expression', 'prop']);
}

export function isEquipped(value: unknown): value is Equipped {
  return isRecord(value) && SLOTS.every((s) => strOrNull(value[s]));
}

export function isCandidateView(value: unknown): value is CandidateView {
  return (
    isRecord(value) &&
    isLook(value.look) &&
    hasStrings(value.labels, ['species', 'palette', 'expression', 'prop']) &&
    isStr(value.starterTop) &&
    isStr(value.name) &&
    isStr(value.blurb) &&
    (value.nameSource === 'llm' || value.nameSource === 'builtin') &&
    isStr(value.svg)
  );
}

export function isCandidateSet(value: unknown): value is CandidateSet {
  return (
    isRecord(value) &&
    typeof value.seed === 'number' &&
    Array.isArray(value.candidates) &&
    value.candidates.every(isCandidateView) &&
    strOrNull(value.fallbackReason)
  );
}

export function isCharacterView(value: unknown): value is CharacterView {
  return (
    isRecord(value) &&
    isStr(value.name) &&
    isStr(value.blurb) &&
    isLook(value.look) &&
    hasStrings(value.labels, ['species', 'palette', 'expression', 'prop']) &&
    isStr(value.stage) &&
    (GROWTH_STAGES as readonly string[]).includes(value.stage) &&
    isStr(value.stageLabel) &&
    typeof value.level === 'number' &&
    isEquipped(value.equipped) &&
    isStr(value.svg) &&
    typeof value.chosenAtMs === 'number'
  );
}

export function isChooseOutcome(value: unknown): value is ChooseOutcome {
  return isRecord(value) && isCharacterView(value.character) && strOrNull(value.toneApplied);
}

export const characterCommands = {
  /** The saved character, or null before the first pick. */
  getCharacter: () => invoke<CharacterView | null>('get_character'),
  /** Rolls candidates for a seed; saves nothing. "다시 뽑기" passes the next seed. */
  rollCharacterCandidates: (input: SetupInput, seed: number) => invoke<CandidateSet>('roll_character_candidates', { input, seed }),
  chooseCharacter: (request: ChooseRequest) => invoke<ChooseOutcome>('choose_character', { request }),
};
