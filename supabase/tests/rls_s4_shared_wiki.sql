-- pgTAP checks for supabase/migrations/20260601000300_shared_wiki.sql. Run with `supabase test db`
-- (needs Docker). NOT run by the agent environment: tracked as V4-1 in docs/stage-4-rooms.md.
-- Cast: A, B and C are room mates, D is a stranger. Everything runs in one transaction, so
-- created_at would tie; `bump` gives each new revision its own later timestamp.
begin;
select plan(22);

insert into auth.users (id, instance_id, aud, role, email)
values ('00000000-0000-4000-8000-00000000000a', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'a@example.test'),
       ('00000000-0000-4000-8000-00000000000b', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'b@example.test'),
       ('00000000-0000-4000-8000-00000000000c', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'c@example.test'),
       ('00000000-0000-4000-8000-00000000000d', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'd@example.test');
insert into public.rooms (id, user_id, name) values ('20000000-0000-4000-8000-000000000001', '00000000-0000-4000-8000-00000000000a', '방');
insert into public.room_members (room_id, user_id, role, nickname)
values ('20000000-0000-4000-8000-000000000001', '00000000-0000-4000-8000-00000000000a', 'owner', '가'),
       ('20000000-0000-4000-8000-000000000001', '00000000-0000-4000-8000-00000000000b', 'member', '나'),
       ('20000000-0000-4000-8000-000000000001', '00000000-0000-4000-8000-00000000000c', 'member', '다');

create temp table ctx (name text primary key, id text) on commit drop;
grant all on ctx to authenticated;

-- Anonymous callers: no table, no RPC.
set local role anon;
select throws_ok($$select * from public.shared_pages$$, '42501', null, 'anon cannot read shared_pages');
select throws_ok($$select * from public.shared_page_revisions$$, '42501', null, 'anon cannot read revisions');
select throws_ok($$select public.list_shared_pages('20000000-0000-4000-8000-000000000001')$$, '42501', null, 'anon cannot list');
reset role;

-- A publishes with no merge consent; B publishes with consent; C publishes with consent.
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000a","role":"authenticated"}';
select throws_ok($$select * from public.shared_page_revisions$$, '42501', null, 'a member cannot read revisions directly');
insert into ctx select 'a1', public.publish_shared_page('20000000-0000-4000-8000-000000000001', '미분', 'A의 글', null, false)->>'revision_id';
select isnt((select id from ctx where name = 'a1'), null, 'A published');
reset role;
update public.shared_page_revisions set created_at = clock_timestamp() where created_at = now();
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000b","role":"authenticated"}';
insert into ctx select 'b1', public.publish_shared_page('20000000-0000-4000-8000-000000000001', '미분', 'B의 글', null, true)->>'revision_id';
reset role;
update public.shared_page_revisions set created_at = clock_timestamp() where created_at = now();
set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000c","role":"authenticated"}';
insert into ctx select 'c1', public.publish_shared_page('20000000-0000-4000-8000-000000000001', '미분', 'C의 글', null, true)->>'revision_id';
reset role;
update public.shared_page_revisions set created_at = clock_timestamp() where created_at = now();
set local role authenticated;

-- A stranger gets nothing and cannot publish.
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000d","role":"authenticated"}';
select is(public.list_shared_pages('20000000-0000-4000-8000-000000000001')->>'code', 'not_member', 'a stranger cannot list');
select is(public.publish_shared_page('20000000-0000-4000-8000-000000000001', 'x', 'y', null, false)->>'code', 'not_member', 'a stranger cannot publish');
select is(public.withdraw_shared_page('20000000-0000-4000-8000-000000000001', '미분')->>'code', 'not_member', 'a stranger cannot withdraw');
select is(public.save_merged_page('20000000-0000-4000-8000-000000000001', '미분', 'z', array[(select id from ctx where name = 'b1')::uuid])->>'code', 'not_member', 'a stranger cannot save a merge');

-- A member sees every member's own latest revision side by side (nobody overwrote anybody).
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000a","role":"authenticated"}';
select is(jsonb_array_length(public.list_shared_pages('20000000-0000-4000-8000-000000000001')->'pages'->0->'revisions'), 3, 'three authors, three revisions');

-- Input limits.
select is(public.publish_shared_page('20000000-0000-4000-8000-000000000001', '미분', repeat('가', 20001), null, false)->>'code', 'too_large', 'oversized body refused');
select is(public.publish_shared_page('20000000-0000-4000-8000-000000000001', '', 'x', null, false)->>'code', 'invalid_input', 'empty title refused');
select is(public.publish_shared_page('20000000-0000-4000-8000-000000000001', '미분', 'x', null, null)->>'code', 'invalid_input', 'the merge flag must be explicit');

-- A concurrent edit: building on the newest revision is fine; building on an older one is stored as a branch.
select is(public.publish_shared_page('20000000-0000-4000-8000-000000000001', '미분', 'A의 새 글', (select id from ctx where name = 'c1')::uuid, false)->>'branched', 'false', 'building on the newest revision is not a branch');
reset role;
update public.shared_page_revisions set created_at = clock_timestamp() where created_at = now();
set local role authenticated;
select is(public.publish_shared_page('20000000-0000-4000-8000-000000000001', '미분', 'A의 또 새 글', (select id from ctx where name = 'a1')::uuid, false)->>'branched', 'true', 'a stale base is stored as a branch');
reset role;
update public.shared_page_revisions set created_at = clock_timestamp() where created_at = now();
set local role authenticated;

-- Merging: only consented sources, and the server credits the authors.
select is(public.save_merged_page('20000000-0000-4000-8000-000000000001', '미분', '합본', array[(select id from ctx where name = 'a1')::uuid, (select id from ctx where name = 'b1')::uuid])->>'code', 'not_consented', 'a source without consent is refused');
insert into ctx select 'm1', public.save_merged_page('20000000-0000-4000-8000-000000000001', '미분', '합본', array[(select id from ctx where name = 'b1')::uuid, (select id from ctx where name = 'c1')::uuid])->>'revision_id';
reset role;
update public.shared_page_revisions set created_at = clock_timestamp() where created_at = now();
set local role authenticated;
select isnt((select id from ctx where name = 'm1'), null, 'a merge of consented sources is saved');
select is((select r->'contributors' from jsonb_array_elements(public.list_shared_pages('20000000-0000-4000-8000-000000000001')->'pages'->0->'revisions') r where r->>'kind' = 'merge'), '["나", "다"]'::jsonb, 'the contributor list comes from the sources');

-- Withdrawing: B's text is gone from the server and the merge crediting B disappears from listings.
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-00000000000b","role":"authenticated"}';
select is(public.withdraw_shared_page('20000000-0000-4000-8000-000000000001', '미분')->>'code', 'ok', 'B withdraws');
select is(public.withdraw_shared_page('20000000-0000-4000-8000-000000000001', '미분')->>'code', 'not_shared', 'a second withdraw has nothing left');
reset role;
select is((select count(*)::int from public.shared_page_revisions where user_id = '00000000-0000-4000-8000-00000000000b' and body <> ''), 0, 'no body of B remains');
select is((select count(*)::int from public.shared_page_revisions where kind = 'merge' and withdrawn_at is null), 0, 'the merge that credited B is tombstoned too');

select * from finish();
rollback;
