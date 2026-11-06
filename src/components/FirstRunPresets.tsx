import { commands } from '../api';
import { useAsync } from '../hooks/useAsync';
import { useStudyLock } from '../hooks/useStudyLock';
import { PresetPanel } from './PresetPanel';

/** Shown above every screen until the first category choice is saved. */
export function FirstRunPresets() {
  const presets = useAsync(commands.getPresetStatus);
  const lock = useStudyLock();
  if (presets.data === null || presets.data.onboarded) return null;
  return (
    <PresetPanel
      firstRun
      editable={lock.allowed}
      onAttempted={() => {
        lock.refresh();
        presets.reload();
      }}
    />
  );
}
