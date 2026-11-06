-- pgTAP checks for supabase/migrations/20260601000200_s4_rooms.sql. Run with `supabase test db`
-- (needs Docker). NOT run by the agent environment: tracked as V4-1 in docs/stage-4-rooms.md.
begin;
select plan(26);

insert into auth.users (id, instance_id, aud, role, email)
values ('00000000-0000-4000-8000-00000000000a', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'a@example.test'),
       ('00000000-0000-4000-8000-00000000000b', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'b@example.test'),
       ('00000000-0000-4000-8000-00000000000c', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'c@example.test');

select is(
  (select count(*)::int from pg_class c join pg_namespace n on n.oid = c.relnamespace
    where n.nspname = 'public' and c.relkind = 'r' and not c.relrowsecurity),
  0, 'every public table has row level security enabled');

-- Anonymous callers see and run nothing.
set local role anon;
select throws_ok($$select * from public.member_status$$, '42501', null, 'anon cannot read member_status');
select throws_ok($$select public.create_room('x', 'y')$$, '42501', null, 'anon cannot create a room');
reset role;

-- A creates a room and an invite; the invite is stored as a hash of a known code for the test.
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000a","role":"authenticated"}';
create temp table ctx (room uuid) on commit drop;
grant all on ctx to authenticated;
insert into ctx select (public.create_room('스터디', '가')->>'room_id')::uuid;
select is(public.list_my_rooms()->>'code', 'ok', 'A can list rooms');
select is(public.create_invite((select room from ctx), 24, 10)->>'code', 'ok', 'A creates an invite');
reset role;

-- Replace the random invite by a known one (table owner only).
update public.room_invites set code_hash = encode(sha256(convert_to('ABCDEFGHJK', 'UTF8')), 'hex');

-- Direct writes are impossible for clients.
set local role authenticated;
select throws_ok($$insert into public.room_members (room_id, user_id, role, nickname)
  values ((select room from ctx), '00000000-0000-4000-8000-00000000000a', 'owner', 'x')$$, '42501', null, 'clients cannot insert room_members');
select throws_ok($$select * from public.room_invites$$, '42501', null, 'clients cannot read invites (hashes) directly');
select throws_ok($$select public.is_room_member((select room from ctx), '00000000-0000-4000-8000-00000000000a')$$, '42501', null, 'internal helper is not callable');

-- B joins: five wrong guesses make the sixth attempt (even with the right code) rate limited.
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000b","role":"authenticated"}';
select is(public.join_room('AAAAAAAAAA', '나')->>'code', 'invalid_code', 'a wrong code is a code, not an error');
select is(public.join_room('AAAAAAAAAB', '나')->>'code', 'invalid_code', 'second wrong code');
select is(public.join_room('AAAAAAAAAC', '나')->>'code', 'invalid_code', 'third wrong code');
select is(public.join_room('AAAAAAAAAD', '나')->>'code', 'invalid_code', 'fourth wrong code');
select is(public.join_room('AAAAAAAAAE', '나')->>'code', 'invalid_code', 'fifth wrong code');
select is(public.join_room('ABCDEFGHJK', '나')->>'code', 'rate_limited', 'failed attempts persisted, so the sixth is limited');
reset role;
delete from public.join_attempts;

set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000b","role":"authenticated"}';
select is(public.join_room('ABCDEFGHJK', '나')->>'code', 'ok', 'B joins with the right code');
select is(public.room_snapshot((select room from ctx))->>'code', 'ok', 'B reads the room through the snapshot');

-- A stranger is not a member and learns nothing.
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000c","role":"authenticated"}';
select is(public.room_snapshot((select room from ctx))->>'code', 'not_member', 'C cannot read the room');
select is(public.create_invite((select room from ctx), 24, 10)->>'code', 'not_owner', 'C cannot create invites');

-- Friend prefs: B hides the level; A (a member) then sees it as null.
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000b","role":"authenticated"}';
select is(public.report_status('studying', 600, 7, 2)->>'code', 'ok', 'B reports status');
select is(public.set_friend_prefs(false, true, false, false, false, false)->>'code', 'ok', 'B hides level');
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000a","role":"authenticated"}';
select is(
  (select m->'level' from jsonb_array_elements(public.room_snapshot((select room from ctx))->'members') m
    where m->>'nickname' = '나'),
  'null'::jsonb, 'A sees B''s level as null');
select is(public.report_daily(current_date, 30, 60)->>'code', 'not_shared', 'week time is refused while not shared');

-- Kick bans.
select is(public.kick_member((select room from ctx), '00000000-0000-4000-8000-00000000000a')->>'code', 'cannot_kick_self', 'owner cannot kick self');
select is(public.kick_member((select room from ctx), '00000000-0000-4000-8000-00000000000b')->>'code', 'ok', 'owner kicks B');
reset role;
delete from public.join_attempts;
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000b","role":"authenticated"}';
select is(public.join_room('ABCDEFGHJK', '나')->>'code', 'banned', 'a kicked member cannot rejoin');
select is(public.room_snapshot((select room from ctx))->>'code', 'not_member', 'a kicked member reads nothing');
reset role;

select * from finish();
rollback;
