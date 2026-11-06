-- pgTAP checks for supabase/migrations/20260601000700_s5_anonymous_guard.sql. Run with
-- `supabase test db` (needs Docker). Tracked as V5-2 in docs/stage-5-verification.md.
-- An anonymous account (the phone: role authenticated + is_anonymous claim) is refused by every
-- owner-side RPC with 'anonymous_not_allowed' and can still run the phone-side ones.
begin;
select plan(23);

insert into auth.users (id, instance_id, aud, role, email, is_anonymous)
values ('00000000-0000-4000-8000-00000000000a', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'a@example.test', false),
       ('00000000-0000-4000-8000-0000000000a1', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', null, true);

select is((select count(*)::int from pg_proc where pronamespace = 'public'::regnamespace and prosrc like '%-- anonymous_guard%'), 36, 'the guard is in 36 functions');

set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-0000000000a1","role":"authenticated","is_anonymous":true}';
select is(public.create_room('스팸', '가')->>'code', 'anonymous_not_allowed', 'anonymous: create_room');
select is(public.join_room('0000000000', '가')->>'code', 'anonymous_not_allowed', 'anonymous: join_room');
select is(public.list_my_rooms()->>'code', 'anonymous_not_allowed', 'anonymous: list_my_rooms');
select is(public.report_status('locked', 60, 1, null)->>'code', 'anonymous_not_allowed', 'anonymous: report_status');
select is(public.set_friend_prefs(true, true, false, false, false, false)->>'code', 'anonymous_not_allowed', 'anonymous: set_friend_prefs');
select is(public.publish_shared_page(gen_random_uuid(), 'p', 'b', null, false)->>'code', 'anonymous_not_allowed', 'anonymous: publish_shared_page');
select is(public.set_profile_settings('가', 'public', 'private', 'private', 'private', 'private', 'private', 'private', 'private', 'private',
  7, '{}', '{}', '{}', '수학')->>'code', 'anonymous_not_allowed', 'anonymous: set_profile_settings');
select is(public.get_my_profile()->>'code', 'anonymous_not_allowed', 'anonymous: get_my_profile');
select is(public.sign_guestbook(gen_random_uuid(), '안녕')->>'code', 'anonymous_not_allowed', 'anonymous: sign_guestbook');
select is(public.send_gift(gen_random_uuid(), gen_random_uuid(), 'hat_beanie', '')->>'code', 'anonymous_not_allowed', 'anonymous: send_gift');
select is(public.revoke_pair()->>'code', 'anonymous_not_allowed', 'anonymous: revoke_pair');
select is(public.phone_status()->>'code', 'anonymous_not_allowed', 'anonymous: phone_status');
select is(public.publish_phone_lock(1, 'r', 'free', 'run_ended', null)->>'code', 'anonymous_not_allowed', 'anonymous: publish_phone_lock');
select is(public.heartbeat(gen_random_uuid(), '1', false), 'anonymous_not_allowed', 'anonymous: heartbeat');
select is(public.replace_plan_snapshot(gen_random_uuid(), 'h', 1, 7, '[]'), 'anonymous_not_allowed', 'anonymous: replace_plan_snapshot');
-- The phone side keeps working: these answer for an unpaired phone, not with the guard code.
select is(public.phone_lease()->>'code', 'not_paired', 'anonymous: phone_lease still runs');
select is(public.phone_beat(1, true, true)->>'code', 'not_paired', 'anonymous: phone_beat still runs');
select is(public.report_phone_pickups('s', 1)->>'code', 'not_paired', 'anonymous: report_phone_pickups still runs');
select isnt(public.claim_pair('0000000000')->>'code', 'anonymous_not_allowed', 'anonymous: claim_pair still runs');

-- A real account is unaffected.
reset role;
select is((select count(*)::int from public.rooms), 0, 'no room was created by the anonymous account');
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000a","role":"authenticated","is_anonymous":false}';
select is(public.create_room('방', '가')->>'code', 'ok', 'a real account still creates a room');
select is(public.list_my_rooms()->>'code', 'ok', 'a real account still lists its rooms');

select * from finish();
rollback;
