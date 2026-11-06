-- pgTAP checks for supabase/migrations/20260601000400_s4p_profile.sql. Run with `supabase test db`
-- (needs Docker). NOT run by the agent environment: tracked as V4-1 in docs/stage-4-rooms.md.
-- Viewer matrix: owner A, room mate B, non-member C, anonymous.
begin;
select plan(27);

insert into auth.users (id, instance_id, aud, role, email)
values ('00000000-0000-4000-8000-00000000000a', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'a@example.test'),
       ('00000000-0000-4000-8000-00000000000b', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'b@example.test'),
       ('00000000-0000-4000-8000-00000000000c', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'c@example.test');

-- A and B share a room; C is a stranger. (Seeded as the table owner.)
insert into public.rooms (id, user_id, name) values ('20000000-0000-4000-8000-000000000001', '00000000-0000-4000-8000-00000000000a', '방');
insert into public.room_members (room_id, user_id, role, nickname)
values ('20000000-0000-4000-8000-000000000001', '00000000-0000-4000-8000-00000000000a', 'owner', '가'),
       ('20000000-0000-4000-8000-000000000001', '00000000-0000-4000-8000-00000000000b', 'member', '나');

-- Anonymous callers cannot read tables or run any RPC except get_public_profile.
set local role anon;
select throws_ok($$select * from public.profile_settings$$, '42501', null, 'anon cannot read profile_settings');
select throws_ok($$select * from public.gifts$$, '42501', null, 'anon cannot read gifts');
select throws_ok($$select public.get_my_profile()$$, '42501', null, 'anon cannot run get_my_profile');
select throws_ok($$select public.send_gift(gen_random_uuid(), gen_random_uuid(), 'hat_beanie', '')$$, '42501', null, 'anon cannot send gifts');
select throws_ok($$select public.profile_view('00000000-0000-4000-8000-00000000000a', null)$$, '42501', null, 'internal helper is not callable');
reset role;

-- A sets up a profile: room public, level friends, wiki private (default), everything else private.
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000a","role":"authenticated"}';
create temp table ctx (slug text) on commit drop;
grant all on ctx to authenticated;
insert into ctx select public.set_profile_settings('가', 'public', 'private', 'friends', 'private', 'friends', 'private', 'private', 'private', 'private',
  7, '{}', '{furn_desk}', '{비밀 페이지}', '수학')->>'slug';
select matches((select slug from ctx), '^[a-f0-9]{16}$', 'a slug is issued');
select is(public.get_my_profile()->'sections'->'wiki', '[]'::jsonb, 'private wiki titles are not stored (owner sees an empty list)');
reset role;

-- Anonymous: only the public section appears.
set local role anon;
select is((select public.get_public_profile((select slug from ctx))->'sections'->>'level'), null, 'anon does not get the friends-only level');
select isnt((select public.get_public_profile((select slug from ctx))->'sections'->'room'), null, 'anon gets the public room');
select is((select count(*)::int from jsonb_object_keys(public.get_public_profile((select slug from ctx))->'sections')), 1, 'anon gets exactly the public sections');
select is(public.get_public_profile('0000000000000000')->>'code', 'not_found', 'unknown slug');
reset role;

-- B (room mate) sees public + friends sections; C sees public only; neither sees private ones.
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000b","role":"authenticated"}';
select is((public.get_public_profile((select slug from ctx))->'sections'->>'level')::int, 7, 'room mate sees the friends level');
select is((public.get_public_profile((select slug from ctx))->'sections'->'wiki')::text, null, 'room mate does not see the private wiki');
select is(public.get_member_profile('00000000-0000-4000-8000-00000000000a')->>'code', 'ok', 'room mate opens the profile by user id');
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000c","role":"authenticated"}';
select is((public.get_public_profile((select slug from ctx))->'sections'->'level')::text, null, 'non-member does not see the friends level');
select is(public.get_member_profile('00000000-0000-4000-8000-00000000000a')->>'code', 'not_member', 'non-member cannot open by user id');
reset role;

-- Visitors are counted once per day per signed-in person.
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000b","role":"authenticated"}';
select lives_ok($$select public.get_public_profile((select slug from ctx))$$, 'B views once');
select lives_ok($$select public.get_public_profile((select slug from ctx))$$, 'B views again the same day');
reset role;
select is((select count(*)::int from public.profile_visits), 2, 'two signed-in visitors (B, C) once each, anonymous not counted');

-- Guestbook: friends only may write, a stranger is refused, rate limited per minute.
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000b","role":"authenticated"}';
select is(public.sign_guestbook('00000000-0000-4000-8000-00000000000a', '안녕')->>'code', 'ok', 'room mate signs');
select is(public.sign_guestbook('00000000-0000-4000-8000-00000000000b', '나')->>'code', 'cannot_self', 'no self entry');
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000c","role":"authenticated"}';
select is(public.sign_guestbook('00000000-0000-4000-8000-00000000000a', '안녕')->>'code', 'not_friends', 'stranger cannot sign');

-- Gifts: catalog only, room mates only, once per idempotency key.
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000b","role":"authenticated"}';
select is(public.send_gift('30000000-0000-4000-8000-000000000001', '00000000-0000-4000-8000-00000000000a', 'acc_knit_scarf', '')->>'code', 'not_giftable', 'achievement items are not giftable');
select is(public.send_gift('30000000-0000-4000-8000-000000000001', '00000000-0000-4000-8000-00000000000a', 'hat_beanie', '축하')->>'code', 'ok', 'gift sent');
select is(public.send_gift('30000000-0000-4000-8000-000000000001', '00000000-0000-4000-8000-00000000000a', 'hat_beanie', '축하')->>'replayed', 'true', 'the same key is not a second gift');
select is(public.gift_status('30000000-0000-4000-8000-000000000009')->>'state', 'none', 'an unknown key reports none');
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000c","role":"authenticated"}';
select is(public.send_gift('30000000-0000-4000-8000-000000000002', '00000000-0000-4000-8000-00000000000a', 'hat_beanie', '')->>'code', 'not_friends', 'a stranger cannot send');
reset role;

select * from finish();
rollback;
