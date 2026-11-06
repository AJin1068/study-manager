import { describe, expect, it } from 'vitest';
import contract from '../api/fixtures/remote-contract.json';
import { isNtfySubscription, isSyncStatus, type SyncStatus } from '../api';
import { parseGrace, signalSummary, svgDataUri, testResultMessage } from './notifyView';

const base: SyncStatus = { active: true, lastBeatOkMs: 1_773_000_000_000, lastSyncOkMs: 1_773_000_005_000, beatError: null, syncError: null, degradedCode: null };

describe('notify contract', () => {
  it('the shared shapes pass the production guards', () => {
    expect(isSyncStatus(contract.syncStatus)).toBe(true);
    expect(isNtfySubscription(contract.ntfySubscription)).toBe(true);
  });
  it('rejects snake_case and missing fields', () => {
    expect(isSyncStatus({ ...contract.syncStatus, lastBeatOkMs: undefined })).toBe(false);
    expect(isSyncStatus({ active: true, last_beat_ok_ms: 1 })).toBe(false);
    expect(isNtfySubscription({ deep_link: 'x', qr_svg: 'y' })).toBe(false);
  });
});

describe('grace input', () => {
  it('accepts 1, 2 and 3', () => {
    for (const n of [1, 2, 3]) expect(parseGrace(String(n))).toEqual({ ok: true, value: n });
    expect(parseGrace(' 2 ')).toEqual({ ok: true, value: 2 });
  });
  it('rejects 0, 4 and non-integers', () => {
    for (const bad of ['0', '4', '10', '', '2.5', '-1', '1e1', 'abc']) expect(parseGrace(bad).ok).toBe(false);
  });
});

describe('signal summary shows three states', () => {
  it('off when inactive: nothing is sent', () => {
    const view = signalSummary({ ...base, active: false });
    expect(view.tone).toBe('off');
    expect(view.details).toEqual([]);
  });
  it('ok with both times', () => {
    const view = signalSummary(base);
    expect(view.tone).toBe('ok');
    expect(view.details).toHaveLength(2);
  });
  it('warn with the reasons, unreachable first', () => {
    const view = signalSummary({ ...base, beatError: 'timeout', syncError: 'weird_code', degradedCode: 'presence_unreachable' });
    expect(view.tone).toBe('warn');
    expect(view.details.join('\n')).toContain('신호를 받지 못');
    expect(view.details.join('\n')).toContain('서버 응답이 없습니다.');
    expect(view.details.join('\n')).toContain('(weird_code)');
  });
  it('says so when nothing has succeeded yet', () => {
    const view = signalSummary({ ...base, lastBeatOkMs: null, lastSyncOkMs: null });
    expect(view.details[0]).toContain('아직');
  });
});

describe('test result and QR', () => {
  it('maps codes to sentences', () => {
    expect(testResultMessage(true, 'ok')).toContain('테스트 알림을 보냈습니다');
    expect(testResultMessage(false, 'unsafe_server')).toContain('안전하지 않아');
    expect(testResultMessage(false, 'http_error')).toContain('오류 응답');
    expect(testResultMessage(false, 'x_y')).toContain('(x_y)');
  });
  it('turns the svg into an inert image address', () => {
    const uri = svgDataUri('<svg><script>alert(1)</script></svg>');
    expect(uri.startsWith('data:image/svg+xml;charset=utf-8,')).toBe(true);
    expect(uri).not.toContain('<');
    expect(decodeURIComponent(uri.split(',')[1])).toBe('<svg><script>alert(1)</script></svg>');
  });
});
