import { useState } from 'react';
import { commands, type Occurrence, type PlanChange, type PlanScope, type UserReasonCode } from '../api';
import {
  USER_REASONS,
  WEEKDAY_LABELS,
  buildCreateInput,
  buildDeleteInput,
  buildUpdateInput,
  derivedEnd,
  toggleWeekday,
  type ReasonDraft,
} from '../pages/calendarLogic';

export type DialogTarget =
  | { kind: 'create'; date: string }
  | { kind: 'edit'; occurrence: Occurrence }
  | { kind: 'delete'; occurrence: Occurrence };

interface Props {
  target: DialogTarget;
  onClose: () => void;
  /** Called with the backend result, including lock rejections, so the caller can show the notice. */
  onResult: (change: PlanChange) => void;
}

function ReasonFields({ value, onChange }: { value: ReasonDraft; onChange: (next: ReasonDraft) => void }) {
  return (
    <fieldset>
      <legend>이유 (필수)</legend>
      {USER_REASONS.map((reason) => (
        <label key={reason.code} className="row">
          <input
            type="radio"
            name="plan-reason"
            checked={value.code === reason.code}
            onChange={() => onChange({ ...value, code: reason.code as UserReasonCode })}
          />
          {reason.label}
        </label>
      ))}
      <input
        type="text"
        aria-label="이유 설명"
        placeholder="이유 설명 (기타는 필수)"
        value={value.text}
        onChange={(e) => onChange({ ...value, text: e.target.value })}
      />
    </fieldset>
  );
}

function WeekdayPicker({ mask, onChange }: { mask: number; onChange: (next: number) => void }) {
  return (
    <div className="row" role="group" aria-label="반복 요일">
      {WEEKDAY_LABELS.map((label, i) => (
        <button
          key={label}
          type="button"
          aria-pressed={(mask & (1 << i)) !== 0}
          onClick={() => onChange(toggleWeekday(mask, i))}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

export function PlanDialog({ target, onClose, onResult }: Props) {
  const occurrence = target.kind === 'create' ? null : target.occurrence;
  const [name, setName] = useState('');
  const [subject, setSubject] = useState('');
  const [date, setDate] = useState(target.kind === 'create' ? target.date : (occurrence?.date ?? ''));
  const [time, setTime] = useState(occurrence?.startTime ?? '20:00');
  const [sets, setSets] = useState(String(occurrence?.sets ?? 3));
  const [weekdays, setWeekdays] = useState(0);
  const [repeatUntil, setRepeatUntil] = useState('');
  const [scope, setScope] = useState<PlanScope>('occurrence');
  const [reason, setReason] = useState<ReasonDraft>({ code: null, text: '' });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit() {
    setError(null);
    let call: Promise<PlanChange>;
    if (target.kind === 'create') {
      const built = buildCreateInput({
        name, subject, startDate: date, startTime: time, sets, weekdays, repeatUntil,
      });
      if (!built.ok) return setError(built.error);
      call = commands.createPlan(built.value);
    } else if (target.kind === 'edit') {
      const built = buildUpdateInput(target.occurrence, {
        scope, newDate: date, newTime: time, sets, weekdays, repeatUntil, reason,
      });
      if (!built.ok) return setError(built.error);
      call = commands.updatePlan(built.value);
    } else {
      const built = buildDeleteInput(target.occurrence, scope, reason);
      if (!built.ok) return setError(built.error);
      call = commands.deletePlan(built.value);
    }
    setBusy(true);
    try {
      onResult(await call);
      onClose();
    } catch (err) {
      setError(`실패: ${String(err)}`);
    } finally {
      setBusy(false);
    }
  }

  const title = target.kind === 'create' ? '계획 만들기' : target.kind === 'edit' ? '계획 고치기' : '계획 지우기';
  const durationMin = occurrence?.durationMin;

  return (
    <section className="card dialog" role="dialog" aria-labelledby="plan-dialog-title">
      <h2 id="plan-dialog-title">{title}</h2>
      {occurrence && (
        <p className="hint">
          {occurrence.name} · {occurrence.subject} · {occurrence.date}{' '}
          {derivedEnd(occurrence.startTime, occurrence.durationMin)}
        </p>
      )}

      {target.kind !== 'create' && (
        <fieldset>
          <legend>적용 범위</legend>
          <label className="row">
            <input type="radio" name="plan-scope" checked={scope === 'occurrence'} onChange={() => setScope('occurrence')} />
            이 회차만
          </label>
          <label className="row">
            <input type="radio" name="plan-scope" checked={scope === 'series'} onChange={() => setScope('series')} />
            이날부터 전체 반복
          </label>
        </fieldset>
      )}

      {target.kind === 'create' && (
        <>
          <label className="row">제목 <input value={name} onChange={(e) => setName(e.target.value)} /></label>
          <label className="row">과목 <input value={subject} onChange={(e) => setSubject(e.target.value)} /></label>
        </>
      )}

      {target.kind !== 'delete' && (
        <>
          {(target.kind === 'create' || scope === 'occurrence') && (
            <label className="row">
              날짜 <input type="date" value={date} onChange={(e) => setDate(e.target.value)} />
            </label>
          )}
          <label className="row">
            시작 <input type="time" value={time} onChange={(e) => setTime(e.target.value)} />
          </label>
          <label className="row">
            세트 수 <input inputMode="numeric" value={sets} onChange={(e) => setSets(e.target.value)} />
          </label>
          {durationMin !== undefined && sets === String(occurrence?.sets) && (
            <p className="hint">끝 시각은 모드 길이로 계산됩니다: {derivedEnd(time, durationMin)}</p>
          )}
          {(target.kind === 'create' || scope === 'series') && (
            <>
              <WeekdayPicker mask={weekdays} onChange={setWeekdays} />
              <label className="row">
                반복 종료일(선택) <input type="date" value={repeatUntil} onChange={(e) => setRepeatUntil(e.target.value)} />
              </label>
              <p className="hint">요일을 고르지 않으면 반복하지 않습니다.</p>
            </>
          )}
        </>
      )}

      {target.kind !== 'create' && <ReasonFields value={reason} onChange={setReason} />}

      {error && <p role="alert" className="error">{error}</p>}
      <div className="row">
        <button type="button" onClick={() => void submit()} disabled={busy}>
          {target.kind === 'delete' ? '지우기' : '저장'}
        </button>
        <button type="button" onClick={onClose} disabled={busy}>취소</button>
      </div>
    </section>
  );
}
