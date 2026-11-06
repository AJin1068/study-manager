import { useEffect, useState } from 'react';
import { commands, type CharacterView } from '../api';
import { CharacterAvatar } from '../components/CharacterAvatar';
import { CharacterSetup } from '../components/CharacterSetup';
import { ShopPanel } from '../components/ShopPanel';

type Loaded = { kind: 'loading' } | { kind: 'error'; message: string } | { kind: 'ready'; view: CharacterView | null };

/** The character, a way to pick again, and the shop. */
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
        </>
      )}
    </main>
  );
}
