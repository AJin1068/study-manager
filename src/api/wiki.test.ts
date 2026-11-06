import { describe, expect, it } from 'vitest';
import fixture from './fixtures/wiki-contract.json';
import { isLlmStatus } from './llm';
import { isAskOutcome, isIngestView, isWikiPage, isWikiSubject } from './wiki';

// The same fixture is serialized from Rust in `wiki::service::tests::wire_shapes_match_the_shared_fixture`.
describe('wiki contract', () => {
  it('fixture values pass the production guards', () => {
    expect(isWikiSubject(fixture.subject)).toBe(true);
    expect(isWikiPage(fixture.page)).toBe(true);
    expect(isIngestView(fixture.ingest)).toBe(true);
    expect(fixture.askOutcomes.every((o: unknown) => isAskOutcome(o))).toBe(true);
    expect(isLlmStatus(fixture.llmStatus)).toBe(true);
  });

  it('rejects snake_case, unknown statuses, and unknown modes', () => {
    expect(isWikiSubject({ ...fixture.subject, page_count: 1, pageCount: undefined })).toBe(false);
    expect(isIngestView({ ...fixture.ingest, status: 'done' })).toBe(false);
    expect(isIngestView({ ...fixture.ingest, proposals: [{ ...fixture.ingest.proposals[0], mode: 'overwrite' }] })).toBe(false);
    expect(isAskOutcome({ status: 'answer', askId: 'a', answer: 'x', citations: 'Derivative', droppedCitations: 0 })).toBe(false);
    expect(isAskOutcome({ status: 'guess' })).toBe(false);
    expect(isLlmStatus({ ...fixture.llmStatus, mode: 'maybe' })).toBe(false);
  });
});
