import { useEffect, useState } from 'react';
import { commands, type CharacterView } from '../api';
import { useAsync } from '../hooks/useAsync';
import { formatAt } from './calendarLogic';
import { CharacterAvatar } from '../components/CharacterAvatar';
import { CharacterSetup } from '../components/CharacterSetup';
import { ShopPanel } from '../components/ShopPanel';

type Loaded = { kind: 'loading' } | { kind: 'error'; message: string } | { kind: 'ready'; view: CharacterView | null };

const loadLedger = () => commands.listPointLedger(20);

/** Recent point earnings and deductions, newest first. */
function LedgerCard() {
  const ledger = useAsync(loadLedger);
  return (
    <section className="card" aria-labelledby="ledger-title">
      <h2 id="ledger-title">포인트 내역</h2>
      {ledger.error !== null && <p className="error">포인트 내역을 읽지 못했습니다: {ledger.error}</p>}
      {ledger.data !== null && ledger.data.length === 0 && <p className="hint">아직 포인트 기록이 없습니다.</p>}
      {ledger.data !== null && ledger.data.length > 0 && (
        <ul className="ledger">
          {ledger.data.map((entry) => (
            <li key={entry.id}>
              <span>
                {entry.reasonCode} <small className="hint">{formatAt(entry.atMs)}</small>
              </span>
              <b className={`mono ${entry.delta < 0 ? 'minus' : 'plus'}`}>
                {entry.delta > 0 ? '+' : ''}
                {entry.delta}P
              </b>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/** The character, a way to pick again, the shop, and the point ledger. */
export function CharacterScreen() {
  const [state, setState] = useState<Loaded>({ kind: 'loading' });
  const [picking, setPicking] = useState(false);

  useEffect(() => {
    commands
      .getCharacter()
      .then((view) => setState({ kind: 'ready', view }))
      .catch((err: unknown) => setState({ kind: 'error', message: String(err) }));
  }, []);

  function show(view: CharacterView) {
    setState({ kind: 'ready', view });
    setPicking(false);
  }

  if (state.kind === 'loading') return <main className="page"><p>불러오는 중</p></main>;
  if (state.kind === 'error') return <main className="page"><p className="error">캐릭터를 읽지 못했습니다: {state.message}</p></main>;
  const view = state.view;
  return (
    <main className="page">
      {(picking || view === null) && <CharacterSetup onDone={show} onCancel={view === null ? undefined : () => setPicking(false)} />}
      {!picking && view !== null && (
        <>
          <section className="card">
            <h2>{view.name}</h2>
            <CharacterAvatar svg={view.svg} label={`${view.name} (${view.stageLabel})`} size={160} />
            <p>{view.blurb}</p>
            <p className="hint">
              Lv.{view.level} {view.stageLabel} · {view.labels.species}
            </p>
            <button type="button" onClick={() => setPicking(true)}>
              캐릭터 다시 정하기
            </button>
          </section>
          <ShopPanel onCharacter={show} />
          <LedgerCard />
        </>
      )}
    </main>
  );
}
