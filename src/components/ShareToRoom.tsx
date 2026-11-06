import { useEffect, useState } from 'react';
import { commands, type RoomSummary, type SharePreview } from '../api';
import { sharedCodeText, WITHDRAW_LIMIT } from '../pages/sharedWikiView';

/** Shares one wiki page with a room. Pages are private until the user confirms here. Mount with `key` = page so state resets per page. */
export function ShareToRoom(props: { pageId: number }) {
  const { pageId } = props;
  const [rooms, setRooms] = useState<RoomSummary[]>([]);
  const [roomId, setRoomId] = useState('');
  const [preview, setPreview] = useState<SharePreview | null>(null);
  const [allowMerge, setAllowMerge] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    commands.listRooms().then((list) => setRooms(list.rooms)).catch(() => undefined);
  }, []);

  if (rooms.length === 0) return null;

  async function run(job: () => Promise<{ code: string }>) {
    try {
      const result = await job();
      setMessage(sharedCodeText(result.code));
      if (result.code === 'ok') setPreview(null);
    } catch (err: unknown) {
      setMessage(String(err));
    }
  }

  return (
    <div className="card">
      <h4>방에 공유</h4>
      <p className="hint">기본은 비공개예요. 아래에서 방을 고르고 확인해야 이 페이지가 올라가요.</p>
      <div className="row">
        <select aria-label="공유할 방" value={roomId} onChange={(e) => {
            setRoomId(e.target.value);
            setPreview(null);
            setAllowMerge(false);
            setMessage(null);
          }}>
          <option value="">방 선택</option>
          {rooms.map((r) => (
            <option key={r.roomId} value={r.roomId}>
              {r.name}
            </option>
          ))}
        </select>
        <button
          type="button"
          disabled={roomId === ''}
          onClick={() => {
            commands.previewShare(pageId, roomId).then(setPreview).catch((err: unknown) => setMessage(String(err)));
          }}
        >
          공유 확인
        </button>
        <button type="button" disabled={roomId === ''} onClick={() => void run(() => commands.withdrawShared(pageId, roomId))}>
          철회
        </button>
      </div>
      {preview !== null && (
        <div role="group" aria-label="공유 확인">
          <p>
            「{preview.pageName}」을(를) {preview.memberCount === null ? '방 멤버' : `방 멤버 ${preview.memberCount}명`}에게 보여요.
          </p>
          <pre>{preview.preview}</pre>
          <p className="hint">{preview.shareNotice}</p>
          <label className="row">
            <input type="checkbox" checked={allowMerge} onChange={(e) => setAllowMerge(e.target.checked)} /> AI 합치기 허용
          </label>
          <p className="hint">{preview.mergeNotice}</p>
          <button type="button" onClick={() => void run(() => commands.sharePage(pageId, roomId, allowMerge, true))}>
            이 내용을 공유하기
          </button>
        </div>
      )}
      <p className="hint">{WITHDRAW_LIMIT}</p>
      {message !== null && <p role="status">{message}</p>}
    </div>
  );
}
