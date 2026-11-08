import { useEffect, useState } from 'react';
import { commands, type CharacterView } from '../api';
import { useAsync } from '../hooks/useAsync';
import { formatAt } from './calendarLogic';
import { pointReasonLabel } from './characterView';
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
      {ledger.data !== null && ledger.data.length === 0 && <p className="hint">아직 포인트 기록이 없어요. 30분 집중하면 첫 포인트가 생겨요.</p>}
      {ledger.data !== null && ledger.data.length > 0 && (
        <ul className="rank">
          {ledger.data.map((entry) => (
            <li key={entry.id}>
              <span>
                {pointReasonLabel(entry.reasonCode)} <small className="hint">{formatAt(entry.atMs)}</small>
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
  const points = useAsync(commands.getPointsStatus);

  useEffect(() => {
    commands
      .getCharacter()
      .then((view) => setState({ kind: 'ready', view }))
      .catch((err: unknown) => setState({ kind: 'error', message: String(err) }));
  }, []);

  function show(view: CharacterView) {
    setState({ kind: 'ready', view });
    setPicking(false);
    points.reload();
  }

  const header = (
    <header>
      <h1>캐릭터</h1>
      <p className="sub">
        {points.data !== null ? (
          <>
            보유 포인트 <b className="mono ink">{points.data.balance.toLocaleString()}P</b> · 친구들에게 지금 캐릭터가 보여요
          </>
        ) : (
          '캐릭터와 상점, 포인트 내역'
        )}
      </p>
    </header>
  );

  if (state.kind === 'loading') return <main className="page">{header}<p className="hint">불러오는 중</p></main>;
  if (state.kind === 'error')
    return (
      <main className="page">
        {header}
        <p className="error">캐릭터를 읽지 못했습니다: {state.message}</p>
      </main>
    );
  const view = state.view;
  return (
    <main className="page">
      {header}
      {(picking || view === null) && <CharacterSetup onDone={show} onCancel={view === null ? undefined : () => setPicking(false)} />}
      {!picking && view !== null && (
        <>
          <section className="card hero" aria-label="내 캐릭터">
            <CharacterAvatar svg={view.svg} label={`${view.name} (${view.stageLabel})`} size={120} />
            <div>
              <div className="level-name">{view.name}</div>
              <p className="hint">
                Lv.{view.level} {view.stageLabel} · {view.labels.species}
              </p>
              <p>{view.blurb}</p>
              <button type="button" onClick={() => setPicking(true)}>
                캐릭터 다시 정하기
              </button>
            </div>
          </section>
          <ShopPanel onCharacter={show} />
          <LedgerCard />
        </>
      )}
    </main>
  );
}
