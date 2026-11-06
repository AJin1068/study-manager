// Mirror of src-tauri/src/commands/wiki.rs. Types match the Rust serde output and are checked
// against src/api/fixtures/wiki-contract.json by both test suites.
// Nothing here writes a page except `applyWikiIngest`, which needs a preview id from an ingest.

import { invoke } from '@tauri-apps/api/core';

export interface WikiSubject {
  id: number;
  slug: string;
  name: string;
  archivedAtMs: number | null;
  createdAtMs: number;
  pageCount: number;
  /** The folder was deleted by hand; writing a page rebuilds it. */
  folderMissing: boolean;
}

export interface WikiPage {
  id: number;
  subjectId: number;
  pageName: string;
  relPath: string;
  contentHash: string;
  updatedAtMs: number;
  quizTotal: number;
  quizCorrect: number;
}

export interface WikiSource {
  id: number;
  subjectId: number;
  kind: string;
  sourceRef: string;
  importedAtMs: number;
  status: string;
}

export interface WikiPageContent {
  pageName: string;
  markdown: string;
}

export interface ReindexReport {
  pages: number;
  added: number;
  updated: number;
  removed: number;
  folderMissing: boolean;
}

export type WikiSourceKind = 'paste' | 'note' | 'link' | 'pdf';

export interface WikiProposal {
  pageName: string;
  markdown: string;
  links: string[];
  /** `append` adds to an existing page after a backup of the old text. */
  mode: 'create' | 'append';
}

export type IngestStatus = 'ready' | 'partial' | 'stopped' | 'empty' | 'unsupported';

export interface SkippedFile {
  name: string;
  reason: string;
}

export interface IngestView {
  previewId: string | null;
  subjectId: number;
  status: IngestStatus;
  processedSteps: number;
  totalSteps: number;
  rejectedSteps: number;
  /** The local heading-based import made the proposals because the LLM is off. */
  usedFallback: boolean;
  stoppedBy: string | null;
  proposals: WikiProposal[];
  skipped: SkippedFile[];
}

export interface ApplyReport {
  written: { pageName: string; mode: 'create' | 'append'; backedUp: boolean }[];
  skipped: { pageName: string; reason: string }[];
}

export interface WikiMatch {
  pageName: string;
  excerpt: string;
}

export type AskOutcome =
  | { status: 'notInWiki'; hint: string }
  | { status: 'answer'; askId: string; answer: string; citations: string[]; droppedCitations: number }
  | { status: 'ungrounded'; matches: WikiMatch[] }
  | { status: 'unavailable'; code: string; matches: WikiMatch[] };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

const isStringArray = (v: unknown): v is string[] => Array.isArray(v) && v.every((x) => typeof x === 'string');
const nullableNumber = (v: unknown): boolean => v === null || typeof v === 'number';

export function isWikiSubject(value: unknown): value is WikiSubject {
  return (
    isRecord(value) &&
    typeof value.id === 'number' &&
    typeof value.slug === 'string' &&
    typeof value.name === 'string' &&
    nullableNumber(value.archivedAtMs) &&
    typeof value.createdAtMs === 'number' &&
    typeof value.pageCount === 'number' &&
    typeof value.folderMissing === 'boolean'
  );
}

export function isWikiPage(value: unknown): value is WikiPage {
  return (
    isRecord(value) &&
    typeof value.id === 'number' &&
    typeof value.subjectId === 'number' &&
    typeof value.pageName === 'string' &&
    typeof value.relPath === 'string' &&
    typeof value.contentHash === 'string' &&
    typeof value.updatedAtMs === 'number' &&
    typeof value.quizTotal === 'number' &&
    typeof value.quizCorrect === 'number'
  );
}

export function isWikiProposal(value: unknown): value is WikiProposal {
  return (
    isRecord(value) &&
    typeof value.pageName === 'string' &&
    typeof value.markdown === 'string' &&
    isStringArray(value.links) &&
    (value.mode === 'create' || value.mode === 'append')
  );
}

const INGEST_STATUSES: readonly string[] = ['ready', 'partial', 'stopped', 'empty', 'unsupported'];

export function isIngestView(value: unknown): value is IngestView {
  return (
    isRecord(value) &&
    (value.previewId === null || typeof value.previewId === 'string') &&
    typeof value.subjectId === 'number' &&
    typeof value.status === 'string' &&
    INGEST_STATUSES.includes(value.status) &&
    typeof value.processedSteps === 'number' &&
    typeof value.totalSteps === 'number' &&
    typeof value.rejectedSteps === 'number' &&
    typeof value.usedFallback === 'boolean' &&
    (value.stoppedBy === null || typeof value.stoppedBy === 'string') &&
    Array.isArray(value.proposals) &&
    value.proposals.every(isWikiProposal) &&
    Array.isArray(value.skipped) &&
    value.skipped.every((f: unknown) => isRecord(f) && typeof f.name === 'string' && typeof f.reason === 'string')
  );
}

function isMatches(value: unknown): value is WikiMatch[] {
  return (
    Array.isArray(value) &&
    value.every((m: unknown) => isRecord(m) && typeof m.pageName === 'string' && typeof m.excerpt === 'string')
  );
}

export function isAskOutcome(value: unknown): value is AskOutcome {
  if (!isRecord(value)) return false;
  switch (value.status) {
    case 'notInWiki':
      return typeof value.hint === 'string';
    case 'answer':
      return (
        typeof value.askId === 'string' &&
        typeof value.answer === 'string' &&
        isStringArray(value.citations) &&
        typeof value.droppedCitations === 'number'
      );
    case 'ungrounded':
      return isMatches(value.matches);
    case 'unavailable':
      return typeof value.code === 'string' && isMatches(value.matches);
    default:
      return false;
  }
}

export const wikiCommands = {
  listWikiSubjects: (includeArchived?: boolean) =>
    invoke<WikiSubject[]>('list_wiki_subjects', { includeArchived: includeArchived ?? false }),
  createWikiSubject: (name: string) => invoke<WikiSubject>('create_wiki_subject', { name }),
  /** Creates the subject when missing, restores it when archived. */
  ensureWikiSubject: (name: string) => invoke<WikiSubject>('ensure_wiki_subject', { name }),
  setWikiSubjectArchived: (id: number, archived: boolean) =>
    invoke<WikiSubject>('set_wiki_subject_archived', { id, archived }),
  wikiSubjectForPlan: (planSubject: string) =>
    invoke<WikiSubject | null>('wiki_subject_for_plan', { planSubject }),
  listWikiPages: (subjectId: number) => invoke<WikiPage[]>('list_wiki_pages', { subjectId }),
  listWikiSources: (subjectId: number) => invoke<WikiSource[]>('list_wiki_sources', { subjectId }),
  readWikiPage: (subjectId: number, pageName: string) =>
    invoke<WikiPageContent>('read_wiki_page', { subjectId, pageName }),
  reindexWiki: (subjectId: number) => invoke<ReindexReport>('reindex_wiki', { subjectId }),
  ingestWikiText: (subjectId: number, kind: WikiSourceKind, label: string, text: string) =>
    invoke<IngestView>('ingest_wiki_text', { subjectId, kind, label, text }),
  ingestWikiFile: (subjectId: number, path: string) => invoke<IngestView>('ingest_wiki_file', { subjectId, path }),
  ingestWikiFolder: (subjectId: number, path: string) =>
    invoke<IngestView>('ingest_wiki_folder', { subjectId, path }),
  draftWikiOutline: (subjectId: number) => invoke<IngestView>('draft_wiki_outline', { subjectId }),
  resumeWikiIngest: (previewId: string) => invoke<IngestView>('resume_wiki_ingest', { previewId }),
  cancelWikiIngest: (previewId: string) => invoke<void>('cancel_wiki_ingest', { previewId }),
  /** `selected` null writes every proposal. */
  applyWikiIngest: (previewId: string, selected: string[] | null) =>
    invoke<ApplyReport>('apply_wiki_ingest', { previewId, selected }),
  askWiki: (subjectId: number, question: string) => invoke<AskOutcome>('ask_wiki', { subjectId, question }),
  previewWikiAnswerPage: (askId: string) => invoke<IngestView>('preview_wiki_answer_page', { askId }),
};
