# Stage 4 — study rooms (S4.1, S4.4–S4.6 + room settings)

Scope of this slice: server schema and RPCs for rooms, membership with invite code/QR, polled
status, rankings, same-time study, friend-visibility settings. The character first-run, MBTI tone,
and shop are other stories.

## Shape

- Server: `supabase/migrations/20260601000200_s4_rooms.sql`. Every write is a `security definer` RPC
  that returns `{"code": ...}` (refusals are returned, never `raise`d, so the attempt record is kept).
  Clients have no insert/update/delete grant on any table.
- Reads: `room_snapshot` / `list_my_rooms` only. The server applies each member's `friend_prefs`, so
  a hidden field arrives as `null`. Other people's rows are never readable through a table.
- Client: `src-tauri/src/rooms/` (ports `port.rs`, adapter `rest.rs`, publisher `status.rs`, polling
  `feed.rs`, pure view code `view.rs`/`ranking.rs`, `together.rs`). `remote/fake/rooms.rs` is the
  in-memory server with the same semantics; `rooms/scenarios.rs` runs multi-user scenarios on it.
- UI: `src/pages/RoomsScreen.tsx` (tab 스터디방), `src/api/rooms.ts`.

## RLS matrix (`fixtures/rooms/rls-matrix.json`)

| table | select | insert/update/delete |
| --- | --- | --- |
| member_status, daily_summaries, friend_prefs, inventory_public | owner (`user_id = auth.uid()`) | none |
| rooms, room_members, room_bans, room_invites, join_attempts, room_schedules, distraction_events | none | none |

Anonymous callers are denied everywhere. Internal helpers `is_room_member`, `ensure_room_owner` have
EXECUTE revoked from public, anon and authenticated.

## Decisions

- Invite: 10 Crockford-base32 characters from the server CSPRNG; only `sha256` is stored; shown once.
  Join is 5 attempts/minute/user and failed attempts persist. 24 h / 10 uses by default; 10 active
  invites per room; owner revokes. Kick = ban. Capacity 8; 5 owned rooms per user.
- Offline: the server sends `age_sec`; 90 s without a report shows as offline on the viewer.
- Polling 5 s while a room is open, 30 s otherwise; a failed poll keeps the last good data.
- Trust (O-21): minutes are bounded (0..1440/day, within 8 days back and 1 day ahead) and a rise larger
  than elapsed minutes + 5 is refused. Values are what each PC reports; they are not proof.
- Friend prefs default to level + state on, everything else off. A share turned off deletes or nulls
  what it covered on the server. During a run a share can be turned off but not on (`tighten_to`).
- Distraction notices carry the sender's nickname and time only, and only when the sender publishes
  and the receiver opted in; notices from before opt-in are not replayed.
- Deviation statistics never leave the PC (not part of any room payload).
- Same-time study: the owner shares up to 20 blocks of the next 7 days (start, name, subject, sets).
  A member previews and copies chosen blocks through the ordinary plan API (lock, overlap check,
  history row); the timetable is re-read from the server at import time.
- User deletion: `rooms.user_id` becomes null; membership rows cascade; the earliest member is
  promoted to owner; an empty room is removed.

## Not verified here

- V4-1: the SQL and pgTAP (`supabase/tests/rls_s4.sql`) were not run (no Supabase CLI/Docker).
  `supabase/tests/rooms.static.test.ts` only checks structure.
- V4-2: no real two-person use against a live server.
- No real Realtime adapter; polling is the only feed.
