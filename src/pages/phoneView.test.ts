import { describe, expect, it } from 'vitest';
import { PHONE_CODES, type PairStatusView, type PhoneLinkView } from '../api';
import { PAIR_NOTICE, REVOKE_NOTICE, codeSecondsLeft, countdownText, pairSummary, phoneCodeText, phoneLinkWarnings } from './phoneView';

const paired = (over: Partial<PairStatusView> = {}): PairStatusView => ({
  paired: true,
  pairedAtMs: 1_000,
  lastBeatMs: 100_000,
  serviceEnabled: true,
  online: true,
  ...over,
});

describe('phone pairing view', () => {
  it('words every code the core can return', () => {
    for (const code of PHONE_CODES) expect(phoneCodeText(code).length, code).toBeGreaterThan(0);
  });

  it('counts down the code to zero and never below', () => {
    expect(codeSecondsLeft(300_000, 0)).toBe(300);
    expect(codeSecondsLeft(300_000, 299_001)).toBe(1);
    expect(codeSecondsLeft(300_000, 300_000)).toBe(0);
    expect(codeSecondsLeft(300_000, 400_000)).toBe(0);
    expect(codeSecondsLeft(null, 0)).toBe(0);
    expect(countdownText(299)).toBe('4:59 남음');
    expect(countdownText(5)).toBe('0:05 남음');
    expect(countdownText(0)).toBe('만료됨');
  });

  it('summarizes an unpaired, a healthy and a worrying pairing', () => {
    expect(pairSummary({ paired: false, pairedAtMs: null, lastBeatMs: null, serviceEnabled: null, online: true }, 0)).toEqual({
      headline: '연결된 폰 없음',
      tone: 'idle',
      details: [],
    });
    expect(pairSummary(paired(), 120_000)).toMatchObject({ headline: '폰 연결됨', tone: 'ok', details: ['폰의 신호가 정상입니다.'] });
    const off = pairSummary(paired({ serviceEnabled: false }), 120_000);
    expect(off.tone).toBe('warn');
    expect(off.details[0]).toContain('차단 서비스가 꺼져');
    const silent = pairSummary(paired(), 100_000 + 91_000);
    expect(silent.tone).toBe('warn');
    expect(silent.details[0]).toContain('분 전');
    expect(pairSummary(paired({ lastBeatMs: null }), 0).details).toEqual(['아직 폰의 신호를 받지 못했습니다.']);
    expect(pairSummary(paired({ online: false }), 120_000).details).toContain('서버에 연결하지 못해 마지막으로 알던 상태입니다.');
  });

  it('tells the user what the QR holds and what ending the pairing does', () => {
    expect(PAIR_NOTICE).toContain('5분');
    expect(PAIR_NOTICE).toContain('앱 이름이나 사용 내역은 보내지 않습니다');
    expect(REVOKE_NOTICE).toContain('긴급 전화');
  });
});

describe('phone link warnings', () => {
  const link = (over: Partial<PhoneLinkView> = {}): PhoneLinkView => ({
    paired: true,
    wantLocked: true,
    published: { seq: 1, state: 'locked', reason: null, atMs: 0 },
    publishFailing: false,
    lastError: null,
    openGaps: [],
    lastBeatMs: 0,
    serviceEnabled: true,
    ...over,
  });

  it('is silent when nothing is paired or nothing is wrong', () => {
    expect(phoneLinkWarnings(link(), 0)).toEqual([]);
    expect(phoneLinkWarnings(link({ paired: false, publishFailing: true, openGaps: [{ kind: 'unreachable', sinceMs: 0 }] }), 0)).toEqual([]);
    expect(phoneLinkWarnings(link({ wantLocked: false, publishFailing: true }), 0)).toEqual([]);
  });

  it('warns that the lock did not reach the server and says the PC itself is fine', () => {
    const lines = phoneLinkWarnings(link({ publishFailing: true }), 0);
    expect(lines[0]).toContain('폰 잠금 신호를 보내지 못하고');
    expect(lines[0]).toContain('PC의 타이머와 차단은 그대로');
  });

  it('words each kind of open gap with how long it has lasted and notes it is statistics only', () => {
    const lines = phoneLinkWarnings(
      link({
        openGaps: [
          { kind: 'unreachable', sinceMs: 0 },
          { kind: 'service_disabled', sinceMs: 120_000 },
          { kind: 'lease_expired', sinceMs: 0 },
        ],
      }),
      5 * 60_000,
    );
    expect(lines).toHaveLength(4);
    expect(lines[0]).toContain('신호가 끊겼습니다');
    expect(lines[0]).toContain('(5분째)');
    expect(lines[1]).toContain('차단 서비스가 꺼져');
    expect(lines[1]).toContain('(3분째)');
    expect(lines[2]).toContain('잠금을 전하지 못한');
    expect(lines[3]).toContain('집중 통계');
    expect(lines[3]).toContain('점수에는 영향이 없습니다');
  });

  it('never shows a negative duration when the clock is behind', () => {
    expect(phoneLinkWarnings(link({ openGaps: [{ kind: 'unreachable', sinceMs: 10_000 }] }), 0)[0]).toContain('(0분째)');
  });
});
