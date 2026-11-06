import { describe, expect, it } from 'vitest';
import fixture from './fixtures/timer-contract.json';
import { isBreakEndingSoon, isRunSummary, isSetSummary, isTimerState } from './timer';

// The same fixture is serialized from Rust in `commands::timer::tests::timer_wire_shapes_match_the_shared_fixture`.
describe('timer contract', () => {
  it('the shared timer states pass the production guard', () => {
    expect(isTimerState(fixture.idleState)).toBe(true);
    expect(isTimerState(fixture.breakState)).toBe(true);
  });

  it('the shared notice and summaries pass the guards, with measured attempt counts', () => {
    expect(isBreakEndingSoon(fixture.breakEndingSoon)).toBe(true);
    expect(isSetSummary(fixture.setSummary)).toBe(true);
    expect(isRunSummary(fixture.runSummary)).toBe(true);
    expect(fixture.setSummary.attemptCount).toBe(3);
    expect(fixture.runSummary.attemptCount).toBe(5);
  });

  it('rejects a summary whose attempt count is missing or not a number', () => {
    expect(isSetSummary({ ...fixture.setSummary, attemptCount: undefined })).toBe(false);
    expect(isSetSummary({ ...fixture.setSummary, attemptCount: '0' })).toBe(false);
    expect(isSetSummary({ ...fixture.setSummary, attemptCount: null })).toBe(false);
    expect(isRunSummary({ ...fixture.runSummary, status: 1 })).toBe(false);
    expect(isBreakEndingSoon({ runId: 1 })).toBe(false);
  });

  it('rejects snake_case phases and a missing lock flag', () => {
    expect(isTimerState({ ...fixture.breakState, phase: 'long_break' })).toBe(false);
    expect(isTimerState({ ...fixture.breakState, lockHeld: undefined })).toBe(false);
    expect(isTimerState({ ...fixture.idleState, next: { planId: 1 } })).toBe(false);
  });
});
