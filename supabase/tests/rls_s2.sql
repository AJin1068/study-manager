-- pgTAP checks for supabase/migrations/20260601000000_s2_core.sql. Run with `supabase test db`
-- (needs Docker). Not run by the agent environment: tracked as V2-8 in docs/stage-2-verification.md.
begin;
select plan(18);

-- Two users and one device each, created as the table owner.
insert into auth.users (id, instance_id, aud, role, email)
values ('00000000-0000-4000-8000-00000000000a', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'a@example.test'),
       ('00000000-0000-4000-8000-00000000000b', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'b@example.test');
insert into public.devices (id, user_id, app_version, platform)
values ('10000000-0000-4000-8000-00000000000a', '00000000-0000-4000-8000-00000000000a', '0.1.0', 'macos'),
       ('10000000-0000-4000-8000-00000000000b', '00000000-0000-4000-8000-00000000000b', '0.1.0', 'windows');

-- RLS is on everywhere.
select is(
  (select count(*)::int from pg_class c join pg_namespace n on n.oid = c.relnamespace
    where n.nspname = 'public' and c.relkind = 'r' and not c.relrowsecurity),
  0, 'every public table has row level security enabled');

-- Anonymous callers see and run nothing.
set local role anon;
select throws_ok($$select * from public.devices$$, '42501', null, 'anon cannot read devices');
select throws_ok($$select public.heartbeat('10000000-0000-4000-8000-00000000000a', '0.1.0', false)$$, '42501', null, 'anon cannot call heartbeat');
reset role;

-- User A.
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000a","role":"authenticated"}';

select is((select count(*)::int from public.devices), 1, 'A sees only their own device');
select is(public.heartbeat('10000000-0000-4000-8000-00000000000a', '0.1.0', true), 'ok', 'A can heartbeat their device');
select is(public.heartbeat('10000000-0000-4000-8000-00000000000b', '0.1.0', true), 'device_not_owned', 'A cannot heartbeat B''s device (code, not error)');
select is((select count(*)::int from public.presence), 1, 'A sees one presence row');

select is(
  public.replace_plan_snapshot('10000000-0000-4000-8000-00000000000a', 'h1', 1000, 14,
    '[{"occurrence_key":"p1:1","start_at_utc_ms":1,"plan_name":"Math","subject":"math","sets":2,"status":"planned"}]'::jsonb),
  'ok', 'snapshot stored');
select is(
  public.replace_plan_snapshot('10000000-0000-4000-8000-00000000000a', 'h1', 1000, 14, '[]'::jsonb),
  'unchanged', 'same hash is a no-op');
select is(
  public.replace_plan_snapshot('10000000-0000-4000-8000-00000000000a', 'h2', 1000, 14, '[{"occurrence_key":"x"}]'::jsonb),
  'invalid_input', 'malformed occurrence returns a code');
select is(
  public.replace_plan_snapshot('10000000-0000-4000-8000-00000000000b', 'h3', 1000, 14, '[]'::jsonb),
  'device_not_owned', 'A cannot replace B''s snapshot');

select throws_ok($$insert into public.plan_occurrences (device_id, occurrence_key, start_at_utc_ms, plan_name, subject, sets, status)
  values ('10000000-0000-4000-8000-00000000000a', 'k', 1, 'n', 's', 1, 'planned')$$, '42501', null, 'clients cannot write plan_occurrences directly');
select throws_ok($$insert into public.devices (id, user_id, app_version, platform)
  values ('10000000-0000-4000-8000-0000000000ff', '00000000-0000-4000-8000-00000000000b', '0.1.0', 'macos')$$,
  '42501', null, 'A cannot create a device owned by B');
select throws_ok($$select public.claim_alert('00000000-0000-4000-8000-00000000000a', 'p1:1', 'missed_plan')$$,
  '42501', null, 'authenticated users cannot call claim_alert');

-- User B cannot see A's mirror.
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000b","role":"authenticated"}';
select is((select count(*)::int from public.plan_occurrences), 0, 'B sees none of A''s occurrences');
select is((select count(*)::int from public.devices), 1, 'B sees only their own device');
reset role;

-- Service role claims exactly once.
set local role service_role;
select is(public.claim_alert('00000000-0000-4000-8000-00000000000a', 'p1:1', 'missed_plan'), 'claimed', 'service role claims the alert');
select is(public.claim_alert('00000000-0000-4000-8000-00000000000a', 'p1:1', 'missed_plan'), 'duplicate', 'second claim is a duplicate');
reset role;

select * from finish();
rollback;
