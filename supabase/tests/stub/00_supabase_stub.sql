-- Minimal stand-in for the parts of a Supabase project the migrations and RLS tests touch:
-- roles, auth.users, auth.uid()/jwt()/role(), and Supabase's default privileges on schema public.
-- It is NOT Supabase: no GoTrue, no PostgREST, no Realtime, no Vault, no pg_cron. Results from
-- this harness are "partial" evidence; only `supabase test db` against the real stack is full.

do $$
begin
  if not exists (select 1 from pg_roles where rolname = 'anon') then create role anon nologin noinherit; end if;
  if not exists (select 1 from pg_roles where rolname = 'authenticated') then create role authenticated nologin noinherit; end if;
  if not exists (select 1 from pg_roles where rolname = 'service_role') then create role service_role nologin noinherit bypassrls; end if;
end $$;

create schema if not exists extensions;
create schema if not exists auth;

create table auth.users (
  id                 uuid primary key,
  instance_id        uuid,
  aud                text,
  role               text,
  email              text,
  is_anonymous       boolean not null default false,
  raw_user_meta_data jsonb,
  created_at         timestamptz not null default now()
);

create function auth.jwt() returns jsonb language sql stable as $$
  select coalesce(nullif(current_setting('request.jwt.claims', true), ''), '{}')::jsonb
$$;
create function auth.uid() returns uuid language sql stable as $$
  select nullif(auth.jwt() ->> 'sub', '')::uuid
$$;
create function auth.role() returns text language sql stable as $$
  select nullif(auth.jwt() ->> 'role', '')
$$;

grant usage on schema public, auth, extensions to anon, authenticated, service_role;
grant execute on all functions in schema auth to anon, authenticated, service_role;

-- Supabase's defaults: new objects in public are open to the API roles; migrations then revoke.
alter default privileges in schema public grant all on tables to anon, authenticated, service_role;
alter default privileges in schema public grant all on sequences to anon, authenticated, service_role;
alter default privileges in schema public grant all on functions to anon, authenticated, service_role;
