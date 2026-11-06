-- pgTAP checks for supabase/migrations/20260601000600_s5_notify_phone_app.sql. Run with
-- `supabase test db` (needs Docker). Not run by the agent environment: tracked as V5-8.
begin;
select plan(4);

insert into auth.users (id, instance_id, aud, role, email)
values ('00000000-0000-4000-8000-0000000000a5', '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', 'a5@example.test');

set local role authenticated;
set local request.jwt.claims = '{"sub":"00000000-0000-4000-8000-0000000000a5","role":"authenticated"}';

select throws_ok($$insert into public.notify_channels (channel) values ('phone_app')$$, '23514', null, 'phone_app without a topic is refused');
select lives_ok($$insert into public.notify_channels (channel, ntfy_topic) values ('phone_app', 'abcdefghijklmnopqrstuvwx')$$, 'phone_app with a topic is stored');
select throws_ok($$update public.notify_channels set ntfy_topic = null where channel = 'phone_app'$$, '23514', null, 'the topic cannot be removed from a phone_app row');
select lives_ok($$update public.notify_channels set channel = 'none', ntfy_topic = null$$, 'switching the channel off clears it');

select * from finish();
rollback;
