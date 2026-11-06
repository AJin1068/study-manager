import { describe, expect, it } from 'vitest';
import fixture from './fixtures/profile-contract.json';
import { isAck, isGiftOverview, isHomeView, isProfileView, isPublishView, isReceiveOutcome, isSendOutcome, SECTIONS } from './profile';

// The same fixture is serialized from Rust in `commands::profile::tests::profile_wire_shapes_match_the_shared_fixture`.
describe('profile contract', () => {
  it('fixture values pass the production guards', () => {
    expect(isHomeView(fixture.home)).toBe(true);
    expect(isAck(fixture.ack)).toBe(true);
    expect(isPublishView(fixture.publish)).toBe(true);
    expect(isProfileView(fixture.view)).toBe(true);
    expect(isProfileView(fixture.hiddenView)).toBe(true);
    expect(isSendOutcome(fixture.sendOutcome)).toBe(true);
    expect(isSendOutcome(fixture.refusedSend)).toBe(true);
    expect(isReceiveOutcome(fixture.receiveOutcome)).toBe(true);
    expect(isGiftOverview(fixture.overview)).toBe(true);
  });

  it('lists every section the home view reports, once, in the Rust order', () => {
    expect(fixture.home.scopes.map((r) => r.section)).toEqual([...SECTIONS]);
  });

  it('rejects snake_case fields, unknown codes, scopes and gift states', () => {
    expect(isAck({ code: 'nope' })).toBe(false);
    expect(isHomeView({ ...fixture.home, scopes: [{ section: 'room', scope: 'everyone' }] })).toBe(false);
    expect(isHomeView({ ...fixture.home, scopes: [{ section: 'attempts', scope: 'public' }] })).toBe(false);
    expect(isProfileView({ ...fixture.view, grass: [{ day: '2026-11-05', study_min: 90 }] })).toBe(false);
    expect(isSendOutcome({ ...fixture.sendOutcome, state: 'lost' })).toBe(false);
    expect(isGiftOverview({ ...fixture.overview, online: 'yes' })).toBe(false);
  });

  it('a hidden profile carries no section at all', () => {
    const hidden = fixture.hiddenView;
    expect(hidden.visible).toEqual([]);
    expect([hidden.room, hidden.grass, hidden.level, hidden.badges, hidden.guestbook, hidden.visitors, hidden.wiki, hidden.subject, hidden.gifts]).toEqual(
      Array(9).fill(null),
    );
  });

  it('no payload type has a field for attempts, apps or points transfer', () => {
    const text = JSON.stringify(fixture).toLowerCase();
    for (const word of ['attempt', 'deviation', 'distraction', 'appname', 'window']) expect(text).not.toContain(word);
  });
});
