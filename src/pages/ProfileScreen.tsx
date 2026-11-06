import { useCallback, useEffect, useState, type FormEvent } from 'react';
import {
  commands,
  SCOPES,
  SECTIONS,
  type GiftOverview,
  type HomeView,
  type MemberView,
  type ProfileView,
  type Scope,
  type Section,
  type ShopItemView,
} from '../api';
import { useAsync } from '../hooks/useAsync';
import {
  codeText,
  giftLine,
  grassCells,
  LINK_NOTICE,
  outboxStateText,
  PRIVACY_NOTICE,
  SCOPE_LABEL,
  scopeChangeText,
  SECTION_LABEL,
  totalMinutes,
} from './profileView';

function localToday(): string {
  const now = new Date();
  const month = String(now.getMonth() + 1).padStart(2, '0');
  const day = String(now.getDate()).padStart(2, '0');
  return `${now.getFullYear()}-${month}-${day}`;
}

function Grass({ view }: { view: ProfileView }) {
  if (view.grass === null) return null;
  const cells = grassCells(view.grass, localToday());
  return (
    <div>
      <p>공부 잔디 · 1년 {totalMinutes(view.grass)}분</p>
      <div className="grass" role="img" aria-label="1년 공부 잔디">
        {cells.map((cell) => (
          <span key={cell.day} className={`grass-cell grass-${cell.level}`} title={`${cell.day} ${cell.studyMin}분`} />
        ))}
      </div>
    </div>
  );
}

function ProfileBody({
  view,
  labelOf,
  ownerId,
  onChanged,
}: {
  view: ProfileView;
  labelOf: (itemId: string) => string;
  ownerId: string | null;
  onChanged: () => void;
}) {
  const [text, setText] = useState('');
  const [note, setNote] = useState<string | null>(null);

  async function sign(event: FormEvent) {
    event.preventDefault();
    if (ownerId === null) return;
    const result = await commands.profileSignGuestbook(ownerId, text);
    setNote(codeText(result.code));
    if (result.code === 'ok') {
      setText('');
      onChanged();
    }
  }

  async function act(run: () => Promise<{ code: Parameters<typeof codeText>[0] }>) {
    const result = await run();
    setNote(codeText(result.code));
    onChanged();
  }

  if (view.code !== 'ok') return <p className="hint">{codeText(view.code)}</p>;
  return (
    <div>
      <h3>{view.name}</h3>
      {view.level !== null && <p>레벨 {view.level}</p>}
      {view.room !== null && (
        <p>
          오두막 {view.room.species ?? '캐릭터'} · {[view.room.hat, view.room.top, view.room.accessory, view.room.hand].filter((x) => x !== null).map((x) => labelOf(x)).join(', ') || '맨몸'}
          {view.room.furniture.length > 0 && ` · 가구 ${view.room.furniture.map(labelOf).join(', ')}`}
        </p>
      )}
      <Grass view={view} />
      {view.badges !== null && <p>배지 {view.badges.length === 0 ? '없음' : view.badges.join(', ')}</p>}
      {view.subject !== null && <p>지금 공부 중: {view.subject}</p>}
      {view.wiki !== null && <p>공개한 위키: {view.wiki.length === 0 ? '없음' : view.wiki.join(', ')}</p>}
      {view.visitors !== null && <p>방문자 오늘 {view.visitors.today}명 · 전체 {view.visitors.total}명</p>}
      {view.gifts !== null && (
        <ul>
          {view.gifts.map((g, i) => (
            <li key={`${g.itemId}-${i}`}>
              {giftLine(g.senderName, labelOf(g.itemId))}
              {g.message !== '' && ` — ${g.message}`}
            </li>
          ))}
        </ul>
      )}
      {view.guestbook !== null && (
        <section aria-label="방명록">
          <h4>방명록</h4>
          <ul>
            {view.guestbook.map((e) => (
              <li key={e.id}>
                <strong>{e.authorName}</strong> {e.body}
                {(ownerId === null || e.mine) && (
                  <button type="button" onClick={() => void act(() => commands.profileDeleteEntry(e.id))}>삭제</button>
                )}
                {ownerId === null && !e.reported && (
                  <button type="button" onClick={() => void act(() => commands.profileReportEntry(e.id))}>신고</button>
                )}
                {e.reported && <span className="hint"> 신고됨</span>}
              </li>
            ))}
          </ul>
          {ownerId !== null && (
            <form onSubmit={(e) => void sign(e)}>
              <input type="text" value={text} maxLength={200} onChange={(e) => setText(e.target.value)} aria-label="방명록 쓰기" />
              <button type="submit" disabled={text.trim() === ''}>남기기</button>
            </form>
          )}
        </section>
      )}
      {note !== null && <p role="status">{note}</p>}
    </div>
  );
}

function Settings({ home, reload }: { home: HomeView; reload: () => void }) {
  const [name, setName] = useState(home.name);
  const [base, setBase] = useState(home.webBaseUrl ?? '');
  const [wiki, setWiki] = useState(home.wikiTitles.join('\n'));
  const [message, setMessage] = useState<string | null>(null);

  async function guarded(run: () => Promise<string>) {
    try {
      setMessage(await run());
    } catch (err: unknown) {
      setMessage(`서버에 연결하지 못했습니다: ${String(err)}`);
    }
    reload();
  }

  const scopeOf = (section: Section): Scope => home.scopes.find((r) => r.section === section)?.scope ?? 'private';

  return (
    <section className="card" aria-labelledby="profile-settings">
      <h2 id="profile-settings">프로필 공개 설정</h2>
      <p className="hint">{PRIVACY_NOTICE}</p>
      <label>
        프로필 이름 (1-20자){' '}
        <input type="text" value={name} maxLength={20} onChange={(e) => setName(e.target.value)} />
      </label>{' '}
      <button type="button" onClick={() => void guarded(async () => codeText((await commands.profileSetName(name)).code))}>저장</button>
      <ul className="scope-list">
        {SECTIONS.map((section) => (
          <li key={section}>
            <label>
              {SECTION_LABEL[section]}{' '}
              <select
                value={scopeOf(section)}
                onChange={(e) => {
                  const scope = SCOPES.find((s) => s === e.target.value);
                  if (scope === undefined) return;
                  void guarded(async () => {
                    const result = await commands.profileSetScope(section, scope);
                    return result.code === 'ok' ? scopeChangeText(section, scope) : codeText(result.code);
                  });
                }}
              >
                {SCOPES.map((scope) => (
                  <option key={scope} value={scope}>{SCOPE_LABEL[scope]}</option>
                ))}
              </select>
            </label>
          </li>
        ))}
      </ul>
      <label>
        공개할 위키 페이지 이름 (한 줄에 하나, 본문은 올라가지 않습니다)
        <textarea value={wiki} rows={3} onChange={(e) => setWiki(e.target.value)} />
      </label>
      <button
        type="button"
        onClick={() =>
          void guarded(async () => {
            const saved = await commands.profileSetWikiTitles(wiki.split('\n'));
            if (saved.code !== 'ok') return codeText(saved.code);
            return codeText((await commands.profilePublish()).code);
          })
        }
      >
        위키 이름 저장
      </button>
      <h3>웹 프로필 링크</h3>
      <p className="hint">{LINK_NOTICE}</p>
      <label>
        웹 번들을 올린 주소 (https){' '}
        <input type="text" value={base} onChange={(e) => setBase(e.target.value)} placeholder="https://example.com/study" />
      </label>{' '}
      <button type="button" onClick={() => void guarded(async () => codeText((await commands.profileSetWebBase(base)).code))}>주소 저장</button>{' '}
      <button type="button" onClick={() => void guarded(async () => codeText((await commands.profilePublish()).code))}>지금 올리기</button>
      {home.link !== null ? <p>내 링크: <code>{home.link}</code></p> : <p className="hint">웹 주소를 저장하고 한 번 올리면 링크가 만들어집니다.</p>}
      {message !== null && <p role="status">{message}</p>}
    </section>
  );
}

function Gifts({ friends, items, labelOf, home }: { friends: MemberView[]; items: ShopItemView[]; labelOf: (id: string) => string; home: HomeView }) {
  const overview = useAsync<GiftOverview>(commands.giftOverview);
  const [to, setTo] = useState('');
  const [itemId, setItemId] = useState('');
  const [message, setMessage] = useState('');
  const [note, setNote] = useState<string | null>(null);
  const giftable = items.filter((i) => i.giftable && i.kind !== 'character');
  const target = friends.find((f) => f.userId === to);
  const item = giftable.find((i) => i.id === itemId);

  async function send() {
    if (target === undefined || item === undefined) return;
    try {
      const result = await commands.giftSend(target.userId, target.nickname, item.id, message);
      setNote(
        result.code === 'ok'
          ? result.state === 'pending'
            ? '전달 결과를 확인하는 중입니다. 확인되기 전에는 포인트를 돌려주지 않습니다'
            : `보냈습니다 (남은 포인트 ${result.balance}P)`
          : codeText(result.code),
      );
      setMessage('');
    } catch (err: unknown) {
      setNote(`서버에 연결하지 못했습니다: ${String(err)}`);
    }
    overview.reload();
  }

  async function answer(giftId: string, accept: boolean) {
    const text = accept ? await commands.giftAccept(giftId) : await commands.giftDecline(giftId);
    setNote(codeText(text.code));
    overview.reload();
  }

  return (
    <section className="card" aria-labelledby="gift-title">
      <h2 id="gift-title">선물</h2>
      <p className="hint">내 포인트로 옷·가구를 사서 같은 방 친구에게 보냅니다. 포인트 자체는 보낼 수 없고, 업적 물건은 선물할 수 없습니다. 하루 {home.giftDailyLimit}개까지.</p>
      <label>
        받는 친구{' '}
        <select value={to} onChange={(e) => setTo(e.target.value)}>
          <option value="">고르세요</option>
          {friends.filter((f) => !f.isSelf).map((f) => (
            <option key={f.userId} value={f.userId}>{f.nickname}</option>
          ))}
        </select>
      </label>{' '}
      <label>
        물건{' '}
        <select value={itemId} onChange={(e) => setItemId(e.target.value)}>
          <option value="">고르세요</option>
          {giftable.map((i) => (
            <option key={i.id} value={i.id}>{i.label} ({i.price}P)</option>
          ))}
        </select>
      </label>{' '}
      <input type="text" value={message} maxLength={home.giftMessageMax} onChange={(e) => setMessage(e.target.value)} placeholder="한마디" aria-label="한마디" />{' '}
      <button type="button" disabled={target === undefined || item === undefined} onClick={() => void send()}>
        {item === undefined ? '보내기' : `${item.price}P로 보내기`}
      </button>
      {note !== null && <p role="status">{note}</p>}
      {overview.error !== null && <p className="error">선물 상자를 읽지 못했습니다: {overview.error}</p>}
      {overview.data !== null && (
        <>
          {!overview.data.online && <p className="hint">서버에 연결되지 않아 보낸 선물 기록만 보여 드립니다.</p>}
          <h3>받은 선물</h3>
          {overview.data.inbox.length === 0 ? <p className="hint">새 선물이 없습니다.</p> : (
            <ul>
              {overview.data.inbox.map((g) => (
                <li key={g.giftId}>
                  {giftLine(g.senderName, labelOf(g.itemId))}{g.message !== '' && ` — ${g.message}`}{' '}
                  <button type="button" onClick={() => void answer(g.giftId, true)}>받기</button>{' '}
                  <button type="button" onClick={() => void answer(g.giftId, false)}>거절</button>
                </li>
              ))}
            </ul>
          )}
          <h3>보낸 선물</h3>
          <ul>
            {overview.data.outbox.map((g) => (
              <li key={g.giftId}>{g.recipientName}에게 {labelOf(g.itemId)} · {outboxStateText(g.state)}</li>
            ))}
          </ul>
        </>
      )}
    </section>
  );
}

/** Profile home: what I show, my page, a friend's page, the guestbook and gifts. */
export function ProfileScreen() {
  const home = useAsync<HomeView>(commands.profileHome);
  const mine = useAsync<ProfileView>(commands.profileMyView);
  const shop = useAsync(commands.getShop);
  const [friends, setFriends] = useState<MemberView[]>([]);
  const [visit, setVisit] = useState<{ id: string; view: ProfileView } | null>(null);

  const { reload: reloadHome } = home;
  const { reload: reloadMine } = mine;
  const reload = useCallback(() => {
    reloadHome();
    reloadMine();
  }, [reloadHome, reloadMine]);

  useEffect(() => {
    commands
      .getRoomFeed()
      .then((feed) => setFriends(feed.room?.members ?? []))
      .catch(() => setFriends([]));
  }, []);

  const items = shop.data?.items ?? [];
  const labelOf = (id: string) => items.find((i) => i.id === id)?.label ?? id;

  async function open(userId: string) {
    try {
      setVisit({ id: userId, view: await commands.profileMemberView(userId) });
    } catch (err: unknown) {
      setVisit(null);
      window.alert(`프로필을 읽지 못했습니다: ${String(err)}`);
    }
  }

  return (
    <main className="page">
      {home.error !== null && <p className="error">프로필을 읽지 못했습니다: {home.error}</p>}
      {home.data !== null && <Settings key={home.data.slug ?? 'new'} home={home.data} reload={reload} />}
      <section className="card" aria-labelledby="my-profile">
        <h2 id="my-profile">내 프로필</h2>
        {mine.error !== null && <p className="hint">서버의 프로필을 읽지 못했습니다 (로그인 또는 연결을 확인하세요).</p>}
        {mine.data !== null && <ProfileBody view={mine.data} labelOf={labelOf} ownerId={null} onChanged={reload} />}
      </section>
      <section className="card" aria-labelledby="friends-profile">
        <h2 id="friends-profile">친구 프로필</h2>
        {friends.filter((f) => !f.isSelf).length === 0 && <p className="hint">같은 방 친구가 있으면 여기서 프로필을 볼 수 있습니다.</p>}
        <ul>
          {friends.filter((f) => !f.isSelf).map((f) => (
            <li key={f.userId}>
              {f.nickname} <button type="button" onClick={() => void open(f.userId)}>방문</button>
            </li>
          ))}
        </ul>
        {visit !== null && (
          <ProfileBody view={visit.view} labelOf={labelOf} ownerId={visit.id} onChanged={() => void open(visit.id)} />
        )}
      </section>
      {home.data !== null && <Gifts friends={friends} items={items} labelOf={labelOf} home={home.data} />}
    </main>
  );
}
