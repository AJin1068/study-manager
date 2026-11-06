import { useCallback, useEffect, useState } from 'react';
import { SharedWikiPanel } from '../components/SharedWikiPanel';
import { commands, onDistractionNotice, type FeedView, type FriendSettings, type InviteOutcome, type RoomAction, type RoomList } from '../api';
import {
  attemptsText,
  canTurnOn,
  codeText,
  degradedText,
  IMPORT_TEXT,
  importable,
  inviteLeft,
  levelText,
  memberSortKey,
  presenceText,
  rankText,
  SETTING_ROWS,
  SHARING_NOTICE,
} from './roomsView';

type Loaded<T> = { kind: 'loading' } | { kind: 'error'; message: string } | { kind: 'ready'; value: T };

/** Study rooms: list, create, join, and one open room with status, ranking and same-time study. */
export function RoomsScreen() {
  const [list, setList] = useState<Loaded<RoomList>>({ kind: 'loading' });
  const [openId, setOpenId] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);

  const refresh = useCallback(() => {
    commands
      .listRooms()
      .then((value) => setList({ kind: 'ready', value }))
      .catch((err: unknown) => setList({ kind: 'error', message: String(err) }));
  }, []);

  useEffect(refresh, [refresh]);

  useEffect(() => {
    let active = true;
    const pending = onDistractionNotice((notice) => {
      if (active) setToast(`${notice.nickname} 님이 딴짓을 시도했어요`);
    });
    return () => {
      active = false;
      pending.then((unlisten) => unlisten()).catch(() => undefined);
    };
  }, []);

  function report(result: RoomAction): boolean {
    setMessage(codeText(result.code));
    if (result.code === 'ok') refresh();
    return result.code === 'ok';
  }

  if (openId !== null) {
    return (
      <main className="page">
        <RoomPanel
          roomId={openId}
          onLeft={() => {
            setOpenId(null);
            refresh();
          }}
          onBack={() => {
            commands.closeRoom().catch(() => undefined);
            setOpenId(null);
            refresh();
          }}
        />
      </main>
    );
  }

  return (
    <main className="page">
      {toast !== null && (
        <p className="hint" role="status">
          {toast} <button type="button" onClick={() => setToast(null)}>닫기</button>
        </p>
      )}
      <section className="card">
        <h2>스터디방</h2>
        {list.kind === 'loading' && <p>불러오는 중</p>}
        {list.kind === 'error' && <p className="error">방 목록을 읽지 못했습니다: {list.message}</p>}
        {list.kind === 'ready' && (
          <>
            {list.value.stale && <p className="hint">서버에 연결하지 못해 마지막으로 받은 목록을 보여 드립니다.</p>}
            {list.value.rooms.length === 0 && <p>아직 들어간 방이 없습니다. 아래에서 만들거나 초대 코드로 들어가세요.</p>}
            <ul>
              {list.value.rooms.map((room) => (
                <li key={room.roomId} className="row">
                  <span>
                    {room.name} · {room.memberCount}/{room.capacity}명 {room.role === 'owner' ? '(방장)' : ''}
                  </span>
                  <button type="button" onClick={() => setOpenId(room.roomId)}>열기</button>
                </li>
              ))}
            </ul>
          </>
        )}
        {message !== null && <p role="status">{message}</p>}
      </section>
      <CreateRoom onResult={report} />
      <JoinRoom onResult={report} />
      <FriendSettingsCard />
    </main>
  );
}

function CreateRoom(props: { onResult: (result: RoomAction) => boolean }) {
  const [name, setName] = useState('');
  const [nickname, setNickname] = useState('');
  const [busy, setBusy] = useState(false);

  async function submit() {
    setBusy(true);
    try {
      if (props.onResult(await commands.createRoom(name, nickname))) {
        setName('');
      }
    } catch (err: unknown) {
      window.alert(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="card">
      <h2>방 만들기</h2>
      <label>
        방 이름 <input value={name} maxLength={40} onChange={(e) => setName(e.target.value)} />
      </label>
      <label>
        내 별명 <input value={nickname} maxLength={20} onChange={(e) => setNickname(e.target.value)} />
      </label>
      <button type="button" disabled={busy || name.trim() === '' || nickname.trim() === ''} onClick={submit}>
        만들기
      </button>
    </section>
  );
}

function JoinRoom(props: { onResult: (result: RoomAction) => boolean }) {
  const [code, setCode] = useState('');
  const [nickname, setNickname] = useState('');
  const [acknowledged, setAcknowledged] = useState(false);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);

  async function paste(text: string) {
    setCode(text);
    // A scanned link carries the code (and the server address, shown if it differs).
    const parsed = await commands.parseInvite(text).catch(() => null);
    if (parsed !== null) {
      setCode(parsed.code);
      setNote(parsed.endpoint === null ? null : `이 초대의 서버: ${parsed.endpoint}`);
    }
  }

  async function submit() {
    setBusy(true);
    try {
      if (props.onResult(await commands.joinRoom(code, nickname, acknowledged))) {
        setCode('');
        setAcknowledged(false);
      }
    } catch (err: unknown) {
      window.alert(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="card">
      <h2>초대 코드로 들어가기</h2>
      <label>
        초대 코드 또는 링크 <input value={code} onChange={(e) => void paste(e.target.value)} autoComplete="off" />
      </label>
      {note !== null && <p className="hint">{note}</p>}
      <label>
        내 별명 <input value={nickname} maxLength={20} onChange={(e) => setNickname(e.target.value)} />
      </label>
      <ul className="hint">
        {SHARING_NOTICE.map((line) => (
          <li key={line}>{line}</li>
        ))}
      </ul>
      <label>
        <input type="checkbox" checked={acknowledged} onChange={(e) => setAcknowledged(e.target.checked)} /> 위 내용을 확인했습니다
      </label>
      <button type="button" disabled={busy || !acknowledged || code.trim() === '' || nickname.trim() === ''} onClick={submit}>
        들어가기
      </button>
    </section>
  );
}

function RoomPanel(props: { roomId: string; onLeft: () => void; onBack: () => void }) {
  const [feed, setFeed] = useState<FeedView | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [invite, setInvite] = useState<InviteOutcome | null>(null);
  const [picked, setPicked] = useState<string[]>([]);
  const { roomId } = props;

  useEffect(() => {
    let active = true;
    let timer: number | undefined;
    const poll = () => {
      commands
        .getRoomFeed()
        .then((view) => {
          if (!active) return;
          setFeed(view);
          timer = window.setTimeout(poll, view.pollIntervalMs);
        })
        .catch(() => {
          if (active) timer = window.setTimeout(poll, 5000);
        });
    };
    commands
      .openRoom(roomId)
      .catch(() => undefined)
      .finally(poll);
    return () => {
      active = false;
      window.clearTimeout(timer);
      commands.closeRoom().catch(() => undefined);
    };
  }, [roomId]);

  async function act(run: () => Promise<RoomAction>, after?: () => void) {
    try {
      const result = await run();
      setMessage(codeText(result.code));
      if (result.code === 'ok') after?.();
    } catch (err: unknown) {
      setMessage(String(err));
    }
  }

  const room = feed?.room ?? null;
  const degraded = degradedText(feed?.degradedCode ?? null);
  if (room === null) {
    return (
      <section className="card">
        <button type="button" onClick={props.onBack}>← 목록</button>
        <p>{degraded ?? '불러오는 중'}</p>
      </section>
    );
  }
  const owner = room.role === 'owner';
  const members = [...room.members].sort((a, b) => memberSortKey(a) - memberSortKey(b));
  const copyable = importable(room.schedule);

  return (
    <>
      <section className="card">
        <button type="button" onClick={props.onBack}>← 목록</button>
        <h2>
          {room.name} · {room.members.length}/{room.capacity}명
        </h2>
        {degraded !== null && <p className="hint" role="status">{degraded}</p>}
        {message !== null && <p role="status">{message}</p>}
        <ul>
          {members.map((m) => (
            <li key={m.userId} className="row">
              <span>
                {m.nickname}
                {m.isSelf ? ' (나)' : ''}
                {m.role === 'owner' ? ' · 방장' : ''} · {levelText(m.level)} · {presenceText(m)}
                {attemptsText(m.attemptsToday) !== null ? ` · ${attemptsText(m.attemptsToday)}` : ''}
                {m.character !== null ? ` · ${m.character.species}` : ''}
              </span>
              {owner && !m.isSelf && (
                <button
                  type="button"
                  onClick={() => {
                    if (window.confirm(`${m.nickname} 님을 내보낼까요? 다시 들어올 수 없습니다.`)) {
                      void act(() => commands.kickMember(roomId, m.userId));
                    }
                  }}
                >
                  내보내기
                </button>
              )}
            </li>
          ))}
        </ul>
        <button
          type="button"
          onClick={() => {
            if (window.confirm('이 방에서 나갈까요?')) void act(() => commands.leaveRoom(roomId), props.onLeft);
          }}
        >
          방 나가기
        </button>
      </section>

      <SharedWikiPanel key={roomId} roomId={roomId} />

      <section className="card">
        <h2>랭킹</h2>
        <h3>오늘</h3>
        <ol>
          {room.todayRanking.map((row) => (
            <li key={row.userId}>{rankText(row)}</li>
          ))}
        </ol>
        <h3>이번 주</h3>
        <ol>
          {room.weekRanking.map((row) => (
            <li key={row.userId}>{rankText(row)}</li>
          ))}
        </ol>
        <p className="hint">시간은 각자의 PC가 보낸 값이며, 서버는 하루 24시간과 갑작스러운 증가만 걸러 냅니다.</p>
      </section>

      <section className="card">
        <h2>같이 공부하기</h2>
        {room.schedule.length === 0 && <p>방장이 공유한 시간표가 없습니다.</p>}
        <ul>
          {room.schedule.map((item) => (
            <li key={item.key} className="row">
              <label>
                <input
                  type="checkbox"
                  disabled={item.inPast || item.alreadyImported}
                  checked={picked.includes(item.key)}
                  onChange={(e) => setPicked(e.target.checked ? [...picked, item.key] : picked.filter((k) => k !== item.key))}
                />{' '}
                {item.localDate} {item.localTime} · {item.name} ({item.subject}, {item.sets}세트)
                {item.alreadyImported ? ' · 넣음' : item.inPast ? ' · 지남' : ''}
              </label>
            </li>
          ))}
        </ul>
        {copyable.length > 0 && (
          <button
            type="button"
            disabled={picked.length === 0}
            onClick={async () => {
              try {
                const results = await commands.importTogether(roomId, picked);
                setMessage(results.map((r) => IMPORT_TEXT[r.outcome]).join(' / '));
                setPicked([]);
              } catch (err: unknown) {
                setMessage(String(err));
              }
            }}
          >
            고른 블록을 내 계획에 넣기
          </button>
        )}
        {owner && (
          <button type="button" onClick={() => void act(() => commands.shareSchedule(roomId))}>
            내 다음 일주일 시간표 공유하기
          </button>
        )}
      </section>

      {owner && (
        <section className="card">
          <h2>초대</h2>
          <button
            type="button"
            onClick={async () => {
              try {
                const made = await commands.createInvite(roomId);
                setInvite(made);
                setMessage(codeText(made.code));
              } catch (err: unknown) {
                setMessage(String(err));
              }
            }}
          >
            새 초대 만들기 (24시간, 10명)
          </button>
          {invite?.inviteCode != null && (
            <div>
              <p>
                초대 코드 <strong>{invite.inviteCode}</strong> (지금만 보입니다)
              </p>
              {invite.qrSvg !== null && (
                <img alt="초대 QR" width={200} height={200} src={`data:image/svg+xml;utf8,${encodeURIComponent(invite.qrSvg)}`} />
              )}
              {invite.deepLink === null && <p className="hint">서버 주소가 설정되지 않아 QR은 만들지 않았습니다. 코드만 전달하세요.</p>}
            </div>
          )}
          <ul>
            {room.invites.map((i) => (
              <li key={i.inviteId} className="row">
                <span>
                  {i.revoked ? '취소됨' : i.expired ? '만료됨' : inviteLeft(i.expiresAtMs, room.fetchedAtMs)} · {i.uses}/{i.maxUses}명 사용
                </span>
                {!i.revoked && !i.expired && (
                  <button type="button" onClick={() => void act(() => commands.revokeInvite(roomId, i.inviteId))}>
                    취소
                  </button>
                )}
              </li>
            ))}
          </ul>
        </section>
      )}
    </>
  );
}

function FriendSettingsCard() {
  const [settings, setSettings] = useState<FriendSettings | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    commands.getFriendSettings().then(setSettings).catch(() => undefined);
  }, []);

  if (settings === null) return null;
  return (
    <section className="card">
      <h2>친구에게 보이는 것</h2>
      <p className="hint">모든 항목은 언제든 끌 수 있고, 끄면 서버에서 해당 값도 지워집니다. 공부 중에는 끄기만 가능합니다.</p>
      {SETTING_ROWS.map((row) => (
        <label key={row.key} className="row">
          <input
            type="checkbox"
            checked={settings[row.key]}
            onChange={async (e) => {
              const next = e.target.checked;
              try {
                const outcome = await commands.updateFriendSetting(row.key, next);
                if (outcome.outcome === 'applied') {
                  setSettings({ ...settings, [row.key]: next });
                  setMessage(null);
                } else {
                  setMessage(canTurnOn(settings, row.key, true) ? '지금은 바꿀 수 없습니다' : '공부 중에는 공개 범위를 넓힐 수 없습니다');
                }
              } catch (err: unknown) {
                setMessage(String(err));
              }
            }}
          />{' '}
          {row.label} <span className="hint">{row.hint}</span>
        </label>
      ))}
      {message !== null && <p role="status">{message}</p>}
    </section>
  );
}
