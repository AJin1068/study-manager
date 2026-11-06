import { useState } from 'react';
import { commands, type CandidateSet, type CandidateView, type CharacterView, type SetupInput } from '../api';
import {
  EMPTY_FORM,
  GENDER_LABEL,
  MBTI_TYPES,
  nameSourceNote,
  nextSeed,
  setupInputFromForm,
  type SetupForm,
} from '../pages/characterView';
import { CharacterAvatar } from './CharacterAvatar';

/**
 * Character setup: MBTI, birth year, and gender are all optional. They stay on this PC (they only
 * shape the candidates and the starting message tone); the model gets looks, never these answers.
 */
export function CharacterSetup(props: { onDone: (view: CharacterView) => void; onCancel?: () => void }) {
  const [form, setForm] = useState<SetupForm>(EMPTY_FORM);
  const [input, setInput] = useState<SetupInput>({});
  const [set, setSet] = useState<CandidateSet | null>(null);
  const [names, setNames] = useState<Record<number, string>>({});
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);

  async function roll(seed: number, nextInput: SetupInput) {
    setBusy(true);
    try {
      const result = await commands.rollCharacterCandidates(nextInput, seed);
      setInput(nextInput);
      setSet(result);
      setNames({});
      setNote(null);
    } catch (err) {
      setNote(`후보를 만들지 못했습니다: ${String(err)}`);
    } finally {
      setBusy(false);
    }
  }

  function start() {
    const parsed = setupInputFromForm(form, new Date().getFullYear());
    if (!parsed.ok) {
      setNote(parsed.message);
      return;
    }
    void roll(0, parsed.input);
  }

  async function choose(candidate: CandidateView, index: number) {
    setBusy(true);
    try {
      const outcome = await commands.chooseCharacter({
        look: candidate.look,
        starterTop: candidate.starterTop,
        name: (names[index] ?? candidate.name).trim(),
        blurb: candidate.blurb,
        input,
      });
      props.onDone(outcome.character);
    } catch (err) {
      setNote(`저장하지 못했습니다: ${String(err)}`);
      setBusy(false);
    }
  }

  return (
    <section className="card" aria-labelledby="char-setup-title">
      <h2 id="char-setup-title">나만의 캐릭터 만들기</h2>
      <p className="hint">모두 건너뛰어도 됩니다. 입력한 내용은 이 컴퓨터에만 저장되고 서버와 친구에게는 보이지 않습니다.</p>
      <div className="row">
        <label>
          MBTI{' '}
          <select value={form.mbti} onChange={(e) => setForm({ ...form, mbti: e.target.value })}>
            <option value="">건너뛰기</option>
            {MBTI_TYPES.map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </select>
        </label>
        <label>
          태어난 해{' '}
          <input
            type="text"
            inputMode="numeric"
            maxLength={4}
            size={6}
            placeholder="건너뛰기"
            value={form.birthYear}
            onChange={(e) => setForm({ ...form, birthYear: e.target.value })}
          />
        </label>
        <label>
          성별{' '}
          <select value={form.gender} onChange={(e) => setForm({ ...form, gender: e.target.value })}>
            <option value="">건너뛰기</option>
            {Object.entries(GENDER_LABEL).map(([value, label]) => (
              <option key={value} value={value}>
                {label}
              </option>
            ))}
          </select>
        </label>
      </div>
      <div className="row">
        <button type="button" disabled={busy} onClick={start}>
          후보 보기
        </button>
        {set !== null && (
          <button type="button" disabled={busy} onClick={() => void roll(nextSeed(set), input)}>
            다시 뽑기
          </button>
        )}
        {props.onCancel && (
          <button type="button" disabled={busy} onClick={props.onCancel}>
            나중에
          </button>
        )}
      </div>
      {note !== null && <p role="status" className="message">{note}</p>}
      {set !== null && (
        <>
          {nameSourceNote(set) !== null && <p className="hint">{nameSourceNote(set)}</p>}
          <div className="character-candidates">
            {set.candidates.map((c, i) => (
              <div key={`${set.seed}-${i}`} className="card">
                <CharacterAvatar svg={c.svg} label={`${c.labels.species} 후보`} />
                <p className="hint">
                  {c.labels.species} · {c.labels.palette} · {c.labels.expression} · {c.labels.prop}
                </p>
                <input
                  type="text"
                  aria-label="이름"
                  maxLength={8}
                  value={names[i] ?? c.name}
                  onChange={(e) => setNames({ ...names, [i]: e.target.value })}
                />
                <p>{c.blurb}</p>
                <button type="button" disabled={busy} onClick={() => void choose(c, i)}>
                  이 친구로 할래요
                </button>
              </div>
            ))}
          </div>
        </>
      )}
    </section>
  );
}
