import { describe, expect, it } from 'vitest';
import { actionLabel, detailLabel, parseGrace } from './enforceView';

describe('enforceView', () => {
  it('labels known actions and passes unknown ones through', () => {
    expect(actionLabel('would_terminate')).toContain('시험 모드');
    expect(actionLabel('site_blocked')).toBe('site_blocked');
  });

  it('labels detail codes and shows nothing when there is none', () => {
    expect(detailLabel(null)).toBe('');
    expect(detailLabel('os_refused')).toBe('OS가 거절함');
    expect(detailLabel('x')).toBe('x');
  });

  it('accepts whole seconds within the range only', () => {
    expect(parseGrace(' 10 ')).toEqual({ ok: true, value: 10 });
    expect(parseGrace('1')).toEqual({ ok: true, value: 1 });
    expect(parseGrace('60')).toEqual({ ok: true, value: 60 });
    for (const bad of ['0', '61', '1.5', '-3', '', 'abc']) {
      expect(parseGrace(bad).ok, bad).toBe(false);
    }
  });
});
