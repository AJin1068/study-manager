import { useEffect, useState } from 'react';
import { commands } from '../api';
import { CharacterSetup } from './CharacterSetup';

/** Shown above every screen until a character is chosen; "나중에" hides it for this session. */
export function FirstRunCharacter() {
  const [needed, setNeeded] = useState(false);

  useEffect(() => {
    commands
      .getCharacter()
      .then((view) => setNeeded(view === null))
      .catch(() => setNeeded(false));
  }, []);

  if (!needed) return null;
  return <CharacterSetup onDone={() => setNeeded(false)} onCancel={() => setNeeded(false)} />;
}
