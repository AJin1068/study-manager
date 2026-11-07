-- The few pgTAP functions the RLS tests call (plan, ok, is, isnt, throws_ok, lives_ok, finish),
-- re-implemented because the pgTAP extension cannot be built on this Windows host. Same call
-- signatures and TAP-style output ("ok N - desc" / "not ok N - desc"). Functions are SECURITY
-- INVOKER so `set local role` in a test applies to the statements under test; only the counter
-- update is SECURITY DEFINER.

create schema if not exists tap;
create table if not exists tap.state (id int primary key default 1, planned int, run int not null default 0, failed int not null default 0);
grant usage on schema tap to public;

create or replace function tap.record(p_ok boolean, p_desc text) returns text
language plpgsql security definer set search_path = pg_catalog, tap as $$
declare n int;
begin
  update tap.state set run = run + 1, failed = failed + (case when p_ok then 0 else 1 end) returning run into n;
  return case when p_ok then 'ok ' else 'not ok ' end || n || coalesce(' - ' || p_desc, '');
end $$;

create or replace function tap.reset(p_planned int) returns void
language plpgsql security definer set search_path = pg_catalog, tap as $$
begin
  delete from tap.state;
  insert into tap.state (id, planned, run, failed) values (1, p_planned, 0, 0);
end $$;

create or replace function tap.summary() returns table (planned int, run int, failed int)
language sql security definer set search_path = pg_catalog, tap as $$ select planned, run, failed from tap.state $$;

create or replace function public.plan(p_planned int) returns text
language plpgsql as $$ begin perform tap.reset(p_planned); return '1..' || p_planned; end $$;

create or replace function public.ok(p_ok boolean, p_desc text default null) returns text
language sql as $$ select tap.record(coalesce(p_ok, false), p_desc) $$;

create or replace function public.is(p_have anyelement, p_want anyelement, p_desc text default null) returns text
language plpgsql as $$
declare r text;
begin
  r := tap.record(p_have is not distinct from p_want, p_desc);
  if r like 'not ok%' then
    r := r || E'\n#   have: ' || coalesce(p_have::text, 'NULL') || E'\n#   want: ' || coalesce(p_want::text, 'NULL');
  end if;
  return r;
end $$;

create or replace function public.isnt(p_have anyelement, p_want anyelement, p_desc text default null) returns text
language sql as $$ select tap.record(p_have is distinct from p_want, p_desc) $$;

create or replace function public.matches(p_have text, p_regex text, p_desc text default null) returns text
language sql as $$ select tap.record(coalesce(p_have ~ p_regex, false), p_desc) $$;

-- errcode is compared against SQLSTATE; errmsg (when given) must equal the message.
create or replace function public.throws_ok(p_sql text, p_errcode text, p_errmsg text default null, p_desc text default null) returns text
language plpgsql as $$
declare st text; msg text;
begin
  begin
    execute p_sql;
  exception when others then
    get stacked diagnostics st = returned_sqlstate, msg = message_text;
    return tap.record(st = p_errcode and (p_errmsg is null or msg = p_errmsg), p_desc)
      || case when st = p_errcode then '' else E'\n#   caught: ' || st || ' ' || msg || E'\n#   wanted: ' || p_errcode end;
  end;
  return tap.record(false, p_desc) || E'\n#   no exception, wanted ' || p_errcode;
end $$;

create or replace function public.lives_ok(p_sql text, p_desc text default null) returns text
language plpgsql as $$
declare st text; msg text;
begin
  begin
    execute p_sql;
  exception when others then
    get stacked diagnostics st = returned_sqlstate, msg = message_text;
    return tap.record(false, p_desc) || E'\n#   died: ' || st || ' ' || msg;
  end;
  return tap.record(true, p_desc);
end $$;

create or replace function public.finish() returns setof text
language plpgsql as $$
declare s record;
begin
  select * into s from tap.summary();
  if s.run <> s.planned then
    return next '# Looks like you planned ' || s.planned || ' tests but ran ' || s.run;
  end if;
  if s.failed > 0 then
    return next '# Looks like you failed ' || s.failed || ' test(s) of ' || s.run;
  end if;
  return next '# done: ' || s.run || ' of ' || s.planned || ' passed';
end $$;

grant execute on all functions in schema tap to public;
