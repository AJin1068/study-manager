import { describe, expect, it } from 'vitest';
import fixture from '../api/fixtures/rooms-contract.json';
import type { FeedView, FriendSettings, RoomCode } from '../api';
import { isFriendSettings } from '../api';
import {
  attemptsText,
  canTurnOn,
  codeText,
  degradedText,
  importable,
  inviteLeft,
  levelText,
  memberSortKey,
  presenceText,
  rankText,
  SETTING_ROWS,
  SHARING_NOTICE,
} from './roomsView';

const feed = fixture.feed as FeedView;
const room = feed.room;
if (room === null) throw new Error('fixture has a room');

describe('rooms view text', () => {
  it('words every server code and never shows the raw code', () => {
    const codes: RoomCode[] = ['ok', 'rate_limited', 'banned', 'room_full', 'cannot_kick_self'];
    for (const code of codes) expect(codeText(code)).not.toContain('_');
  });

  it('describes presence, including a hidden member', () => {
    expect(presenceText(room.members[0])).toBe('공부 중 · 10분 남음');
    expect(presenceText(room.members[1])).toBe('비공개');
    expect(levelText(room.members[1].level)).toBe('레벨 비공개');
    expect(levelText(4)).toBe('Lv.4');
    expect(attemptsText(null)).toBeNull();
    expect(attemptsText(2)).toBe('오늘 시도 2회');
  });

  it('ranks with ties by position and marks members who do not share', () => {
    expect(rankText(room.todayRanking[0])).toBe('1위 가 · 30분');
    expect(rankText(room.todayRanking[1])).toBe('나 · 비공개');
  });

  it('shows invite time left and expiry', () => {
    expect(inviteLeft(3_600_000, 0)).toBe('1시간 남음');
    expect(inviteLeft(10, 10)).toBe('만료됨');
  });

  it('degraded text separates sign-in problems from connection problems', () => {
    expect(degradedText(null)).toBeNull();
    expect(degradedText('auth_expired')).toContain('로그인');
    expect(degradedText('timeout')).toContain('연결');
  });

  it('only future, not-yet-copied blocks can be imported', () => {
    const base = room.schedule[0];
    const items = [base, { ...base, key: 'a', inPast: true }, { ...base, key: 'b', alreadyImported: true }];
    expect(importable(items).map((i) => i.key)).toEqual([base.key]);
  });

  it('studying members sort first after the viewer', () => {
    const order = [...room.members].sort((a, b) => memberSortKey(a) - memberSortKey(b)).map((m) => m.nickname);
    expect(order).toEqual(['가', '나']);
  });
});

describe('friend settings card', () => {
  const settings = fixture.friendSettings as FriendSettings;

  it('lists every setting the core has, once', () => {
    expect(isFriendSettings(settings)).toBe(true);
    expect(SETTING_ROWS.map((r) => r.key).sort()).toEqual(Object.keys(settings).sort());
  });

  it('while studying a share can be switched off but not on', () => {
    expect(canTurnOn(settings, 'shareLevel', true)).toBe(true);
    expect(canTurnOn(settings, 'shareWeekTime', true)).toBe(false);
    expect(canTurnOn(settings, 'shareWeekTime', false)).toBe(true);
  });

  it('the notice names what is never sent', () => {
    const text = SHARING_NOTICE.join(' ');
    for (const word of ['창 제목', '입력한 글', '성격 유형', '이탈 기록']) expect(text).toContain(word);
  });
});
