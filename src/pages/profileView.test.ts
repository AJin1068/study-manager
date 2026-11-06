import { describe, expect, it } from 'vitest';
import { SCOPES, SECTIONS } from '../api';
import {
  codeText,
  giftLine,
  grassCells,
  grassLevel,
  outboxStateText,
  PRIVACY_NOTICE,
  SCOPE_LABEL,
  scopeChangeText,
  SECTION_LABEL,
  totalMinutes,
} from './profileView';

describe('profile view helpers', () => {
  it('words every section, scope and outcome', () => {
    for (const s of SECTIONS) expect(SECTION_LABEL[s].length).toBeGreaterThan(0);
    for (const s of SCOPES) expect(SCOPE_LABEL[s].length).toBeGreaterThan(0);
    expect(codeText('not_friends')).toContain('같은 방');
    expect(outboxStateText('pending')).toContain('환불하지 않고');
    expect(outboxStateText('refunded')).toContain('환불됨');
  });

  it('says that deviation data is never public', () => {
    expect(PRIVACY_NOTICE).toContain('딴짓 횟수');
    expect(PRIVACY_NOTICE).toContain('비공개');
    expect(Object.values(SECTION_LABEL).join(' ')).not.toContain('딴짓');
  });

  it('warns that narrowing deletes server data', () => {
    expect(scopeChangeText('wiki', 'private')).toContain('지웁니다');
    expect(scopeChangeText('wiki', 'friends')).toContain('친구만');
  });

  it('levels the grass at the minute boundaries', () => {
    expect([0, 1, 29, 30, 89, 90, 179, 180, 1440].map(grassLevel)).toEqual([0, 1, 1, 2, 2, 3, 3, 4, 4]);
  });

  it('builds one cell per day ending today and fills the gaps', () => {
    const cells = grassCells([{ day: '2026-11-05', studyMin: 90 }], '2026-11-06', 3);
    expect(cells).toEqual([
      { day: '2026-11-04', level: 0, studyMin: 0 },
      { day: '2026-11-05', level: 3, studyMin: 90 },
      { day: '2026-11-06', level: 0, studyMin: 0 },
    ]);
    expect(grassCells([], '2026-11-06')).toHaveLength(365);
    expect(grassCells([], 'not a day')).toEqual([]);
  });

  it('sums minutes and words a gift line', () => {
    expect(totalMinutes([{ day: 'a', studyMin: 5 }, { day: 'b', studyMin: 7 }])).toBe(12);
    expect(giftLine('민수', '목도리')).toBe('민수님이 준 목도리');
  });
});
