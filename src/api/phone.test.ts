import { describe, expect, it } from 'vitest';
import fixture from './fixtures/phone-contract.json';
import { PHONE_CODES, isPairStart, isPairStatusView, isPhoneAck, isPhoneCode, isPhoneLinkView } from './phone';

// The same fixture is serialized from Rust in `commands::phone::tests::phone_wire_shapes_match_the_shared_fixture`.
describe('phone contract', () => {
  it('fixture values pass the production guards', () => {
    expect(isPairStart(fixture.start)).toBe(true);
    expect(isPairStart(fixture.refusedStart)).toBe(true);
    expect(isPairStatusView(fixture.status)).toBe(true);
    expect(isPairStatusView(fixture.unpaired)).toBe(true);
    expect(isPhoneAck(fixture.ack)).toBe(true);
    expect(isPhoneLinkView(fixture.link)).toBe(true);
    expect(isPhoneLinkView(fixture.linkIdle)).toBe(true);
  });

  it('rejects a link view with an unknown gap kind, snake_case or a wrong type', () => {
    expect(isPhoneLinkView({ ...fixture.link, openGaps: [{ kind: 'app_usage', sinceMs: 1 }] })).toBe(false);
    expect(isPhoneLinkView({ ...fixture.link, want_locked: true, wantLocked: undefined })).toBe(false);
    expect(isPhoneLinkView({ ...fixture.link, published: { ...fixture.link.published, state: 'maybe' } })).toBe(false);
    expect(isPhoneLinkView({ ...fixture.link, publishFailing: 'no' })).toBe(false);
  });

  it('carries only ids, times and flags in the link view', () => {
    expect(Object.keys(fixture.link).sort()).toEqual(
      ['lastBeatMs', 'lastError', 'openGaps', 'paired', 'publishFailing', 'published', 'serviceEnabled', 'wantLocked'].sort(),
    );
  });

  it('rejects snake_case fields, unknown codes and wrong types', () => {
    expect(isPhoneCode('nope')).toBe(false);
    expect(isPairStart({ ...fixture.start, code: 'nope' })).toBe(false);
    expect(isPairStart({ ...fixture.start, display_code: 'x', displayCode: undefined })).toBe(false);
    expect(isPairStatusView({ ...fixture.status, online: 'yes' })).toBe(false);
    expect(isPairStatusView({ ...fixture.status, serviceEnabled: 1 })).toBe(false);
    expect(isPhoneAck({ code: 7 })).toBe(false);
  });

  it('lists every server code of the shared RPC contract', async () => {
    const contract = (await import('../../fixtures/phone/rpc-contract.json')).default as { rpcs: { returns: string[] }[] };
    const returned = new Set(contract.rpcs.flatMap((r) => r.returns));
    for (const code of returned) expect(PHONE_CODES as readonly string[], code).toContain(code);
  });

  it('carries no field for app names, usage or the code itself in the status', () => {
    expect(Object.keys(fixture.status).sort()).toEqual(['lastBeatMs', 'online', 'pairedAtMs', 'paired', 'serviceEnabled'].sort());
  });
});
