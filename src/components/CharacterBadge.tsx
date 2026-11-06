import { commands } from '../api';
import { useAsync } from '../hooks/useAsync';
import { CharacterAvatar } from './CharacterAvatar';

/** The character with its level, for the main screen and the dashboard. Nothing before the first pick. */
export function CharacterBadge() {
  const character = useAsync(commands.getCharacter);
  if (character.error !== null) return <p className="error">캐릭터를 읽지 못했습니다: {character.error}</p>;
  const view = character.data;
  if (view === null) return null;
  return (
    <section className="card character-badge" aria-label="내 캐릭터">
      <div className="row">
        <CharacterAvatar svg={view.svg} label={`${view.name} (${view.stageLabel})`} size={72} />
        <div>
          <strong>{view.name}</strong>
          <p className="hint">
            Lv.{view.level} {view.stageLabel}
          </p>
        </div>
      </div>
    </section>
  );
}
