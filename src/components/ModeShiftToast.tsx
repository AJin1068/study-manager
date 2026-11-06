import { useState } from 'react';
import { commands } from '../api';
import { failureNotice, undoNotice, type ModeNotice } from '../pages/modeShiftNotice';

interface Props {
  notice: ModeNotice;
  /** Replaces the shown notice (after an undo attempt). */
  onNotice: (notice: ModeNotice) => void;
  /** Called after the undo reached the core, applied or not, so the parent can re-read state. */
  onUndoAttempted: () => void;
}

/** Status line for a mode change, with a one-click undo when plans were moved. */
export function ModeShiftToast({ notice, onNotice, onUndoAttempted }: Props) {
  const [busy, setBusy] = useState(false);
  const batchId = notice.undoBatchId;

  async function undo(id: string) {
    setBusy(true);
    try {
      onNotice(undoNotice(await commands.undoPlanBatch(id)));
    } catch (err) {
      onNotice(failureNotice(err));
    } finally {
      setBusy(false);
      onUndoAttempted();
    }
  }

  return (
    <p role="status" className="message">
      {notice.text}
      {batchId !== null && (
        <button type="button" className="undo" disabled={busy} onClick={() => void undo(batchId)}>
          되돌리기
        </button>
      )}
    </p>
  );
}
