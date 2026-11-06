-- Stage 2 core schema: devices, plan mirror, presence, notification channel, alert log.
--
-- Model: the desktop app is the only writer of plans (local SQLite is the source of truth); this
-- schema is a read-only mirror for the phone and for server-side detection of a missed plan.
-- Privacy: only plan names, subjects, times, and repeat expansions are stored. There is no column
-- for window titles, URLs, typed text, focus detail, or block events.
--
-- Conventions
--   * Every table has user_id (default auth.uid()) and row level security enabled.
--   * Clients never write plan_snapshots / plan_occurrences / presence / alert_log directly; they
--     call the RPCs below. Server time (now()) is the only clock for presence.
--   * RPC failures that must be counted or reported come back as a returned code, not as RAISE:
--     a RAISE rolls the transaction back and would erase the record of the failed attempt.
--   * Every security definer function pins search_path, revokes EXECUTE from PUBLIC and anon, and
--     grants it only to the roles that need it. Owner checks use auth.uid() inside the function.
--   * No secret appears in this file. The cron job that calls the Edge Function reads its
--     credential from Vault at run time (see supabase/README.md).

create extension if not exists pgcrypto;

-- ---------------------------------------------------------------- tables

create table public.devices (
  id          uuid primary key,
  user_id     uuid not null default auth.uid() references auth.users (id) on delete cascade,
  app_version text not null check (length(app_version) <= 40),
  platform    text not null check (platform in ('macos', 'windows', 'other')),
  created_at  timestamptz not null default now()
);
create index devices_user on public.devices (user_id);

create table public.plan_snapshots (
  user_id         uuid not null default auth.uid() references auth.users (id) on delete cascade,
  device_id       uuid not null references public.devices (id) on delete cascade,
  snapshot_hash   text not null,
  horizon_days    integer not null check (horizon_days between 1 and 60),
  generated_at_ms bigint not null,
  updated_at      timestamptz not null default now(),
  primary key (device_id)
);
create index plan_snapshots_user on public.plan_snapshots (user_id);

create table public.plan_occurrences (
  id              bigint generated always as identity primary key,
  user_id         uuid not null default auth.uid() references auth.users (id) on delete cascade,
  device_id       uuid not null references public.devices (id) on delete cascade,
  occurrence_key  text not null check (length(occurrence_key) <= 120),
  start_at_utc_ms bigint not null,
  plan_name       text not null check (length(plan_name) <= 120),
  subject         text not null check (length(subject) <= 120),
  sets            integer not null check (sets >= 1),
  status          text not null check (status in ('planned', 'skipped', 'canceled', 'moved')),
  unique (device_id, occurrence_key)
);
create index plan_occurrences_user_start on public.plan_occurrences (user_id, start_at_utc_ms);

create table public.plan_history_mirror (
  id            bigint generated always as identity primary key,
  user_id       uuid not null default auth.uid() references auth.users (id) on delete cascade,
  device_id     uuid not null references public.devices (id) on delete cascade,
  local_id      bigint not null,
  plan_local_id bigint,
  at_ms         bigint not null,
  action        text not null,
  reason_code   text not null,
  reason_text   text,
  before_json   text,
  after_json    text,
  outcome       text not null check (outcome in ('applied', 'rejected_locked')),
  batch_id      text,
  unique (device_id, local_id)
);
create index plan_history_mirror_user_at on public.plan_history_mirror (user_id, at_ms);

create table public.presence (
  device_id    uuid primary key references public.devices (id) on delete cascade,
  user_id      uuid not null default auth.uid() references auth.users (id) on delete cascade,
  last_seen_at timestamptz not null default now(),
  app_version  text not null,
  studying     boolean not null default false
);
create index presence_user on public.presence (user_id);

create table public.notify_channels (
  user_id          uuid primary key default auth.uid() references auth.users (id) on delete cascade,
  channel          text not null default 'none' check (channel in ('none', 'ntfy', 'telegram', 'phone_app')),
  -- The ntfy topic works like a password: whoever knows it can read the notifications. Owner-only RLS.
  ntfy_topic       text check (ntfy_topic is null or length(ntfy_topic) between 16 and 64),
  telegram_chat_id text check (telegram_chat_id is null or length(telegram_chat_id) <= 40),
  missed_grace_min integer not null default 3 check (missed_grace_min between 1 and 3),
  updated_at       timestamptz not null default now()
);

create table public.alert_log (
  id             bigint generated always as identity primary key,
  user_id        uuid not null references auth.users (id) on delete cascade,
  occurrence_key text not null,
  kind           text not null check (kind in ('missed_plan')),
  claimed_at     timestamptz not null default now(),
  unique (user_id, occurrence_key, kind)
);

-- ---------------------------------------------------------------- row level security

alter table public.devices             enable row level security;
alter table public.plan_snapshots      enable row level security;
alter table public.plan_occurrences    enable row level security;
alter table public.plan_history_mirror enable row level security;
alter table public.presence            enable row level security;
alter table public.notify_channels     enable row level security;
alter table public.alert_log           enable row level security;

-- Table privileges mirror the matrix in fixtures/sync/rls-matrix.json: anon gets nothing,
-- authenticated gets only the operations a policy exists for.
revoke all on public.devices, public.plan_snapshots, public.plan_occurrences,
  public.plan_history_mirror, public.presence, public.notify_channels, public.alert_log
  from public, anon, authenticated;

grant select, insert, update, delete on public.devices         to authenticated;
grant select                         on public.plan_snapshots  to authenticated;
grant select                         on public.plan_occurrences to authenticated;
grant select, insert                 on public.plan_history_mirror to authenticated;
grant select                         on public.presence        to authenticated;
grant select, insert, update, delete on public.notify_channels to authenticated;
grant select                         on public.alert_log       to authenticated;

create policy devices_select on public.devices for select to authenticated using (user_id = auth.uid());
create policy devices_insert on public.devices for insert to authenticated with check (user_id = auth.uid());
create policy devices_update on public.devices for update to authenticated using (user_id = auth.uid()) with check (user_id = auth.uid());
create policy devices_delete on public.devices for delete to authenticated using (user_id = auth.uid());

create policy plan_snapshots_select on public.plan_snapshots for select to authenticated using (user_id = auth.uid());
create policy plan_occurrences_select on public.plan_occurrences for select to authenticated using (user_id = auth.uid());

create policy plan_history_mirror_select on public.plan_history_mirror for select to authenticated using (user_id = auth.uid());
create policy plan_history_mirror_insert on public.plan_history_mirror for insert to authenticated
  with check (
    user_id = auth.uid()
    and exists (select 1 from public.devices d where d.id = device_id and d.user_id = auth.uid())
  );

create policy presence_select on public.presence for select to authenticated using (user_id = auth.uid());

create policy notify_channels_select on public.notify_channels for select to authenticated using (user_id = auth.uid());
create policy notify_channels_insert on public.notify_channels for insert to authenticated with check (user_id = auth.uid());
create policy notify_channels_update on public.notify_channels for update to authenticated using (user_id = auth.uid()) with check (user_id = auth.uid());
create policy notify_channels_delete on public.notify_channels for delete to authenticated using (user_id = auth.uid());

create policy alert_log_select on public.alert_log for select to authenticated using (user_id = auth.uid());

-- ---------------------------------------------------------------- RPCs

-- Replaces this device's mirrored plan occurrences. The client expands repeats (14 days ahead);
-- the server never interprets a repeat rule. Same hash twice is a no-op, so retries are free.
create or replace function public.replace_plan_snapshot(
  p_device_id uuid,
  p_snapshot_hash text,
  p_generated_at_ms bigint,
  p_horizon_days integer,
  p_occurrences jsonb
) returns text
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_item jsonb;
begin
  if v_user is null then
    return 'device_not_owned';
  end if;
  if not exists (select 1 from public.devices d where d.id = p_device_id and d.user_id = v_user) then
    return 'device_not_owned';
  end if;
  if p_snapshot_hash is null or p_generated_at_ms is null or p_horizon_days is null
     or p_horizon_days not between 1 and 60
     or p_occurrences is null or jsonb_typeof(p_occurrences) <> 'array'
     or jsonb_array_length(p_occurrences) > 5000 then
    return 'invalid_input';
  end if;
  if exists (select 1 from public.plan_snapshots s where s.device_id = p_device_id and s.snapshot_hash = p_snapshot_hash) then
    return 'unchanged';
  end if;

  -- A malformed element (for example a non-numeric start) must come back as a code, not an error.
  begin
    for v_item in select * from jsonb_array_elements(p_occurrences) loop
      if jsonb_typeof(v_item) <> 'object'
         or (v_item ->> 'occurrence_key') is null
         or (v_item ->> 'start_at_utc_ms') is null
         or (v_item ->> 'plan_name') is null
         or (v_item ->> 'subject') is null
         or coalesce((v_item ->> 'sets')::integer, 0) < 1
         or ((v_item ->> 'start_at_utc_ms')::bigint) is null
         or (v_item ->> 'status') not in ('planned', 'skipped', 'canceled', 'moved') then
        return 'invalid_input';
      end if;
    end loop;
    if (select count(distinct e ->> 'occurrence_key') from jsonb_array_elements(p_occurrences) e) <> jsonb_array_length(p_occurrences) then
      return 'invalid_input';
    end if;
  exception when others then
    return 'invalid_input';
  end;

  delete from public.plan_occurrences where device_id = p_device_id and user_id = v_user;
  insert into public.plan_occurrences (user_id, device_id, occurrence_key, start_at_utc_ms, plan_name, subject, sets, status)
  select v_user, p_device_id, e ->> 'occurrence_key', (e ->> 'start_at_utc_ms')::bigint,
         e ->> 'plan_name', e ->> 'subject', (e ->> 'sets')::integer, e ->> 'status'
  from jsonb_array_elements(p_occurrences) e;

  insert into public.plan_snapshots (user_id, device_id, snapshot_hash, horizon_days, generated_at_ms, updated_at)
  values (v_user, p_device_id, p_snapshot_hash, p_horizon_days, p_generated_at_ms, now())
  on conflict (device_id) do update
    set snapshot_hash = excluded.snapshot_hash, horizon_days = excluded.horizon_days,
        generated_at_ms = excluded.generated_at_ms, updated_at = now();
  return 'ok';
end;
$$;

-- "The PC is on" signal. The server stamps now(); the PC clock is never trusted.
create or replace function public.heartbeat(
  p_device_id uuid,
  p_app_version text,
  p_studying boolean
) returns text
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
begin
  if v_user is null or not exists (select 1 from public.devices d where d.id = p_device_id and d.user_id = v_user) then
    return 'device_not_owned';
  end if;
  insert into public.presence (device_id, user_id, last_seen_at, app_version, studying)
  values (p_device_id, v_user, now(), left(coalesce(p_app_version, ''), 40), coalesce(p_studying, false))
  on conflict (device_id) do update
    set last_seen_at = now(), app_version = excluded.app_version, studying = excluded.studying;
  return 'ok';
end;
$$;

-- Records that a missed-plan alert for this occurrence was sent, exactly once. Called only by the
-- Edge Function with the service role; an authenticated user cannot mint or hide alerts.
create or replace function public.claim_alert(
  p_user_id uuid,
  p_occurrence_key text,
  p_kind text
) returns text
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_inserted integer;
begin
  insert into public.alert_log (user_id, occurrence_key, kind)
  values (p_user_id, p_occurrence_key, p_kind)
  on conflict (user_id, occurrence_key, kind) do nothing;
  get diagnostics v_inserted = row_count;
  return case when v_inserted = 1 then 'claimed' else 'duplicate' end;
end;
$$;

revoke execute on function public.replace_plan_snapshot(uuid, text, bigint, integer, jsonb) from public, anon;
revoke execute on function public.heartbeat(uuid, text, boolean) from public, anon;
revoke execute on function public.claim_alert(uuid, text, text) from public, anon, authenticated;

grant execute on function public.replace_plan_snapshot(uuid, text, bigint, integer, jsonb) to authenticated;
grant execute on function public.heartbeat(uuid, text, boolean) to authenticated;
grant execute on function public.claim_alert(uuid, text, text) to service_role;

-- ---------------------------------------------------------------- scheduling (human step)
-- The missed-plan check runs once a minute. Run this by hand after deploying the Edge Function
-- `missed-plan` and storing its credential in Vault (see supabase/README.md). It is commented out
-- so a plain `supabase db push` never schedules anything and no secret is ever written here.
--
-- select cron.schedule(
--   'missed-plan-check', '* * * * *',
--   $cron$
--     select net.http_post(
--       url     := (select decrypted_secret from vault.decrypted_secrets where name = 'missed_plan_url'),
--       headers := jsonb_build_object(
--         'Content-Type', 'application/json',
--         'Authorization', 'Bearer ' || (select decrypted_secret from vault.decrypted_secrets where name = 'missed_plan_token')
--       ),
--       body    := '{}'::jsonb
--     );
--   $cron$
-- );
