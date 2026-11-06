import { describe, expect, it } from 'vitest';
import fixture from './fixtures/rooms-contract.json';
import {
  isDistractionNotice,
  isFeedView,
  isFriendSettings,
  isImportResult,
  isInviteOutcome,
  isRoomAction,
  isRoomList,
} from './rooms';

// The same fixture is serialized from Rust in `commands::rooms::tests::room_wire_shapes_match_the_shared_fixture`.
describe('rooms contract', () => {
  it('fixture values pass the production guards', () => {
    expect(isRoomList(fixture.list)).toBe(true);
    expect(isRoomAction(fixture.action)).toBe(true);
    expect(isInviteOutcome(fixture.invite)).toBe(true);
    expect(isFeedView(fixture.feed)).toBe(true);
    expect(isFriendSettings(fixture.friendSettings)).toBe(true);
    expect(isImportResult(fixture.importResult)).toBe(true);
    expect(isDistractionNotice(fixture.notice)).toBe(true);
  });

  it('accepts a feed with no room yet and a degraded one', () => {
    expect(isFeedView({ ...fixture.feed, room: null, degradedCode: 'timeout', lastOkMs: null })).toBe(true);
  });

  it('rejects snake_case fields, unknown codes and unknown presence kinds', () => {
    expect(isRoomAction({ code: 'nope', roomId: null })).toBe(false);
    expect(isRoomList({ ...fixture.list, rooms: [{ room_id: 'r1' }] })).toBe(false);
    const room = fixture.feed.room;
    const member = { ...room.members[0], presence: { kind: 'sleeping', remainingSec: null } };
    expect(isFeedView({ ...fixture.feed, room: { ...room, members: [member] } })).toBe(false);
    expect(isImportResult({ ...fixture.importResult, outcome: 'done' })).toBe(false);
  });

  it('a hidden member carries no level, state detail or character', () => {
    const hidden = fixture.feed.room.members[1];
    expect([hidden.level, hidden.attemptsToday, hidden.character, hidden.presence.kind]).toEqual([null, null, null, 'hidden']);
  });
});
