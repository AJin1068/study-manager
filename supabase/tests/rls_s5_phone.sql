-- pgTAP checks for supabase/migrations/20260601000500_s5_phone.sql. Run with `supabase test db`
-- (needs Docker). NOT run by the agent environment: tracked as V5-2 in docs/stage-5-verification.md.
-- Actors: owner A, phone P (anonymous sign-in: role authenticated + is_anonymous claim), a second
-- phone Q that is never paired, and a second owner B.
begin;
select plan(52);

insert into auth.users (id, instance_id, aud, role, email, is_anonymous)
values ('00000000-0000-4000-8000-00000000000a', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'a@example.test', false),
       ('00000000-0000-4000-8000-00000000000b', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'b@example.test', false),
       ('00000000-0000-4000-8000-0000000000a1', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', null, true),
       ('00000000-0000-4000-8000-0000000000a2', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', null, true);

create temp table ctx (name text primary key, value text) on commit drop;
grant all on ctx to authenticated;

-- Callers without a JWT (role anon) see and run nothing.
set local role anon;
select throws_ok($$select * from public.phone_lock_state$$, '42501', null, 'anon cannot read the lease');
select throws_ok($$select public.claim_pair('ABCDEFGHJK')$$, '42501', null, 'anon cannot claim a pairing');
select throws_ok($$select public.phone_lease()$$, '42501', null, 'anon cannot read a lease through the RPC');
reset role;

-- A phone (anonymous account) cannot make codes; the owner can.
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-0000000000a1","role":"authenticated","is_anonymous":true}';
select is(public.create_pair_code()->>'code', 'anonymous_not_allowed', 'a phone cannot make a pairing code');
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000a","role":"authenticated","is_anonymous":false}';
insert into ctx select 'code', public.create_pair_code()->>'pair_code';
select matches((select value from ctx where name = 'code'), '^[0-9A-HJKMNP-TV-Z]{10}$', 'the owner gets a 10-character code');
reset role;
select is((select count(*)::int from public.pair_codes where code_hash = (select value from ctx where name = 'code')), 0, 'only a hash of the code is stored');
select is((select count(*)::int from public.pair_codes), 1, 'one code row exists');

-- Wrong guesses are counted even though they fail: five in a minute, then the correct code is refused.
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-0000000000a1","role":"authenticated","is_anonymous":true}';
select is(public.claim_pair('0000000000')->>'code', 'invalid_code', 'a wrong code is invalid');
select is(public.claim_pair('not a code')->>'code', 'invalid_code', 'a malformed code is invalid');
select is(public.claim_pair('1111111111')->>'code', 'invalid_code', 'a third wrong code');
select is(public.claim_pair('2222222222')->>'code', 'invalid_code', 'a fourth wrong code');
select is(public.claim_pair('3333333333')->>'code', 'invalid_code', 'a fifth wrong code');
select is(public.claim_pair((select value from ctx where name = 'code'))->>'code', 'rate_limited', 'the sixth attempt in a minute is limited even with the right code');
reset role;
select is((select count(*)::int from public.pair_attempts where user_id = '00000000-0000-4000-8000-0000000000a1'), 5, 'the attempts were kept (no rollback)');
delete from public.pair_attempts;

-- The phone claims the code once.
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-0000000000a1","role":"authenticated","is_anonymous":true}';
select is(public.claim_pair((select lower(substr(value, 1, 5) || '-' || substr(value, 6)) from ctx where name = 'code'))->>'code', 'ok', 'the code works typed with a dash and lower case');
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-0000000000a2","role":"authenticated","is_anonymous":true}';
select is(public.claim_pair((select value from ctx where name = 'code'))->>'code', 'used', 'a code cannot be reused by anybody');
reset role;

-- Expiry and self-claim use a second owner B.
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000b","role":"authenticated","is_anonymous":false}';
delete from ctx;
insert into ctx select 'code', public.create_pair_code()->>'pair_code';
select is(public.claim_pair((select value from ctx where name = 'code'))->>'code', 'cannot_self', 'an owner cannot pair with itself');
reset role;
update public.pair_codes set expires_at = now() - interval '1 second' where owner_user_id = '00000000-0000-4000-8000-00000000000b';
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-0000000000a2","role":"authenticated","is_anonymous":true}';
select is(public.claim_pair((select value from ctx where name = 'code'))->>'code', 'expired', 'an expired code is refused');
reset role;

-- The lease: only the owner publishes; sequence numbers rise; length is capped.
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000a","role":"authenticated","is_anonymous":false}';
select is(public.publish_phone_lock(1, 'run-1', 'locked', null, (extract(epoch from now()) * 1000)::bigint + 600000)->>'code', 'ok', 'a lease is published');
select is(public.publish_phone_lock(0, 'run-1', 'locked', null, (extract(epoch from now()) * 1000)::bigint + 600000)->>'code', 'invalid_input', 'seq 0 is refused');
select is(public.publish_phone_lock(2, 'run-1', 'locked', null, (extract(epoch from now()) * 1000)::bigint + 4 * 3600000)->>'code', 'lease_too_long', 'a lease longer than the cap is refused');
select is(public.publish_phone_lock(2, 'run-1', 'locked', 'run_ended', (extract(epoch from now()) * 1000)::bigint + 600000)->>'code', 'invalid_input', 'a locked lease cannot carry a release cause');
select is(public.publish_phone_lock(2, 'run-1', 'free', null, 0)->>'code', 'invalid_input', 'a free lease needs a cause');
select is(public.publish_phone_lock(1, 'run-1', 'free', 'run_ended', 0)->>'code', 'stale_seq', 'the same seq with other content is stale');
select is(public.publish_phone_lock(1, 'run-1', 'locked', null, (select valid_until_ms from public.phone_lock_state))->>'replayed', 'true', 'a retry of the same publication is a no-op');
reset role;

-- Reads: the paired phone sees the owner's rows, the unpaired phone and the other owner do not.
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-0000000000a1","role":"authenticated","is_anonymous":true}';
select is((select count(*)::int from public.phone_lock_state), 1, 'the paired phone reads the lease');
select is(public.phone_lease()->'lease'->>'seq', '1', 'the lease RPC returns it with the server clock');
select is(public.phone_beat(1, true, true)->>'code', 'ok', 'the phone sends a heartbeat');
select is(public.report_phone_pickups('run-1:1', 3)->>'code', 'ok', 'the phone reports pick-ups');
select is(public.report_phone_pickups('run-1:1', 3)->>'code', 'ok', 'the same report again is accepted');
select throws_ok($$insert into public.phone_lock_state (owner_user_id, seq, run_id, state, release_cause, issued_at_ms, valid_until_ms)
  values ('00000000-0000-4000-8000-0000000000a1', 9, 'x', 'free', 'run_ended', 1, 1)$$, '42501', null, 'the phone cannot write the lease table');
select throws_ok($$update public.phone_lock_state set state = 'free'$$, '42501', null, 'the phone cannot update the lease');
select throws_ok($$insert into public.phone_pairs (owner_user_id, phone_user_id) values ('00000000-0000-4000-8000-00000000000b', '00000000-0000-4000-8000-0000000000a1')$$, '42501', null, 'a phone cannot bind itself to another owner');
select throws_ok($$select * from public.pair_codes$$, '42501', null, 'codes are not readable');
select throws_ok($$select public.phone_owner_of('00000000-0000-4000-8000-0000000000a1')$$, '42501', null, 'the internal helper is not callable');
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-0000000000a2","role":"authenticated","is_anonymous":true}';
select is((select count(*)::int from public.phone_lock_state), 0, 'an unpaired phone reads nothing');
select is(public.phone_lease()->>'code', 'not_paired', 'an unpaired phone has no lease');
select is(public.phone_beat(1, true, true)->>'code', 'not_paired', 'an unpaired phone cannot beat');
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000b","role":"authenticated","is_anonymous":false}';
select is((select count(*)::int from public.phone_lock_state), 0, 'another owner reads nothing');
select is((select count(*)::int from public.phone_beats), 0, 'another owner sees no heartbeats');
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000a","role":"authenticated","is_anonymous":false}';
select is((select count(*)::int from public.phone_beats), 1, 'the owner sees the heartbeat');
select is((select count(*)::int from public.phone_pickups), 1, 'the owner sees one pick-up row for the repeated report');
select is(public.phone_status()->>'paired', 'true', 'the owner sees the pairing');
reset role;

-- Revoking ends access at once, although the phone still holds a valid JWT.
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000a","role":"authenticated","is_anonymous":false}';
select is(public.revoke_pair()->>'code', 'ok', 'the owner revokes the pairing');
select is(public.revoke_pair()->>'code', 'not_found', 'revoking twice finds nothing');
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-0000000000a1","role":"authenticated","is_anonymous":true}';
select is((select count(*)::int from public.phone_lock_state), 0, 'a revoked phone reads no lease');
select is((select count(*)::int from public.phone_pickups), 0, 'a revoked phone reads no pick-ups');
select is(public.phone_lease()->>'code', 'not_paired', 'a revoked phone has no lease RPC');
select is(public.phone_beat(1, true, true)->>'code', 'not_paired', 'a revoked phone cannot beat');
select is(public.report_phone_pickups('run-1:1', 4)->>'code', 'not_paired', 'a revoked phone cannot report');
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000a","role":"authenticated","is_anonymous":false}';
select is(public.publish_phone_lock(2, 'run-1', 'free', 'pair_revoked', 0)->>'code', 'ok', 'the owner can still publish a free lease');
select is(public.create_pair_code()->>'code', 'ok', 'after revoking the owner can pair again');
reset role;

select * from finish();
rollback;
