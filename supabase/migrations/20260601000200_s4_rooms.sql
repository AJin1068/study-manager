-- Stage 4 (S4.1/S4.4/S4.5/S4.6): study rooms, invites, member status, daily summaries, friend
-- visibility, content-free distraction events, and the shared timetable.
--
-- Who may see what (friends = members of the same room, O-11)
--   * Clients can NOT read other people's rows from any table. Every table is switched on for row
--     level security, and the only policies are "your own row" selects on four tables. Reading a
--     room goes through the RPC room_snapshot, which applies each member's friend_prefs on the
--     server, so a hidden field is never sent to anyone else (it arrives as JSON null).
--   * All writes go through the RPCs below. There is no INSERT/UPDATE/DELETE policy or grant.
--   * Deviation statistics stay private by default: attempts are shown only when that member turned
--     friend_prefs.share_attempts on, and the distraction event carries no app name, no title, no
--     count: only "this member got distracted" to receivers who opted in.
--   * Nothing here can hold a window title, URL, typed text, MBTI, birth year, or gender.
--
-- Conventions follow the stage-2 migration: security definer + pinned search_path, EXECUTE revoked
-- from PUBLIC and anon, owner checks with auth.uid(), and failures that must be counted come back
-- as a returned code (never RAISE, which would roll back the record of the attempt). Every RPC
-- returns jsonb {"code": "...", ...}.
--
-- User deletion policy (cascade)
--   * auth.users row deleted -> that user's room_members, room_bans, room_invites they created,
--     join_attempts, member_status, daily_summaries, friend_prefs, inventory_public and
--     distraction_events rows are deleted. Rooms are NOT deleted with their creator: rooms.user_id
--     becomes NULL. A room left without an owner promotes its earliest-joined member the next time
--     anyone lists or opens it (ensure_room_owner). A room with no members left is deleted by
--     leave_room; one emptied by account deletion stays as an unreachable empty row.
--   * Leaving as the owner passes ownership to the earliest-joined remaining member; the last member
--     leaving deletes the room (members, bans, invites, schedule cascade).
--
-- Invite codes: 10 characters from the 32-letter Crockford alphabet (about 50 bits) built from
-- gen_random_uuid(), which draws from the server's CSPRNG. Only sha256(code) is stored; the code
-- is returned once by create_invite and never again.

-- ---------------------------------------------------------------- tables

create table public.rooms (
  id         uuid primary key default gen_random_uuid(),
  user_id    uuid default auth.uid() references auth.users (id) on delete set null,
  name       text not null check (length(name) between 1 and 40),
  capacity   integer not null default 8 check (capacity between 2 and 20),
  created_at timestamptz not null default now()
);

create table public.room_members (
  room_id   uuid not null references public.rooms (id) on delete cascade,
  user_id   uuid not null default auth.uid() references auth.users (id) on delete cascade,
  role      text not null check (role in ('owner', 'member')),
  nickname  text not null check (length(nickname) between 1 and 20),
  joined_at timestamptz not null default now(),
  primary key (room_id, user_id)
);
create index room_members_user on public.room_members (user_id);

create table public.room_bans (
  room_id   uuid not null references public.rooms (id) on delete cascade,
  user_id   uuid not null references auth.users (id) on delete cascade,
  banned_at timestamptz not null default now(),
  primary key (room_id, user_id)
);

create table public.room_invites (
  id         uuid primary key default gen_random_uuid(),
  room_id    uuid not null references public.rooms (id) on delete cascade,
  user_id    uuid not null default auth.uid() references auth.users (id) on delete cascade,
  code_hash  text not null unique check (length(code_hash) = 64),
  expires_at timestamptz not null,
  max_uses   integer not null check (max_uses between 1 and 50),
  uses       integer not null default 0 check (uses >= 0),
  revoked_at timestamptz,
  created_at timestamptz not null default now()
);
create index room_invites_room on public.room_invites (room_id);

create table public.join_attempts (
  id      bigint generated always as identity primary key,
  user_id uuid not null default auth.uid() references auth.users (id) on delete cascade,
  occurred_at timestamptz not null default now()
);
create index join_attempts_user_at on public.join_attempts (user_id, occurred_at);

create table public.member_status (
  user_id        uuid primary key default auth.uid() references auth.users (id) on delete cascade,
  state          text not null check (state in ('studying', 'break', 'idle')),
  remaining_sec  integer not null check (remaining_sec between 0 and 86400),
  level          integer not null check (level between 1 and 999),
  attempts_today integer check (attempts_today is null or attempts_today >= 0),
  updated_at     timestamptz not null default now()
);

create table public.daily_summaries (
  user_id    uuid not null default auth.uid() references auth.users (id) on delete cascade,
  day        date not null,
  study_min  integer not null check (study_min between 0 and 1440),
  plan_min   integer not null check (plan_min between 0 and 1440),
  updated_at timestamptz not null default now(),
  primary key (user_id, day)
);

create table public.friend_prefs (
  user_id             uuid primary key default auth.uid() references auth.users (id) on delete cascade,
  share_level         boolean not null default true,
  share_status        boolean not null default true,
  share_week_time     boolean not null default false,
  share_attempts      boolean not null default false,
  distraction_publish boolean not null default false,
  distraction_receive boolean not null default false,
  updated_at          timestamptz not null default now()
);

create table public.inventory_public (
  user_id        uuid primary key default auth.uid() references auth.users (id) on delete cascade,
  species        text not null check (species ~ '^[a-z0-9_]{1,40}$'),
  slot_hat       text check (slot_hat is null or slot_hat ~ '^[a-z0-9_]{1,40}$'),
  slot_top       text check (slot_top is null or slot_top ~ '^[a-z0-9_]{1,40}$'),
  slot_accessory text check (slot_accessory is null or slot_accessory ~ '^[a-z0-9_]{1,40}$'),
  slot_hand      text check (slot_hand is null or slot_hand ~ '^[a-z0-9_]{1,40}$'),
  updated_at     timestamptz not null default now()
);

create table public.room_schedules (
  room_id    uuid primary key references public.rooms (id) on delete cascade,
  user_id    uuid not null default auth.uid() references auth.users (id) on delete cascade,
  items      jsonb not null default '[]'::jsonb,
  updated_at timestamptz not null default now()
);

create table public.distraction_events (
  id      bigint generated always as identity primary key,
  user_id uuid not null default auth.uid() references auth.users (id) on delete cascade,
  occurred_at timestamptz not null default now()
);
create index distraction_events_at on public.distraction_events (occurred_at);

-- ---------------------------------------------------------------- row level security

alter table public.rooms              enable row level security;
alter table public.room_members       enable row level security;
alter table public.room_bans          enable row level security;
alter table public.room_invites       enable row level security;
alter table public.join_attempts      enable row level security;
alter table public.member_status      enable row level security;
alter table public.daily_summaries    enable row level security;
alter table public.friend_prefs       enable row level security;
alter table public.inventory_public   enable row level security;
alter table public.room_schedules     enable row level security;
alter table public.distraction_events enable row level security;

-- Table privileges mirror fixtures/rooms/rls-matrix.json: anon gets nothing, authenticated may
-- read only its own member_status, daily_summaries, friend_prefs and inventory_public rows.
revoke all on public.rooms, public.room_members, public.room_bans, public.room_invites,
  public.join_attempts, public.member_status, public.daily_summaries, public.friend_prefs,
  public.inventory_public, public.room_schedules, public.distraction_events
  from public, anon, authenticated;

grant select on public.member_status    to authenticated;
grant select on public.daily_summaries  to authenticated;
grant select on public.friend_prefs     to authenticated;
grant select on public.inventory_public to authenticated;

create policy member_status_select on public.member_status for select to authenticated using (user_id = auth.uid());
create policy daily_summaries_select on public.daily_summaries for select to authenticated using (user_id = auth.uid());
create policy friend_prefs_select on public.friend_prefs for select to authenticated using (user_id = auth.uid());
create policy inventory_public_select on public.inventory_public for select to authenticated using (user_id = auth.uid());

-- ---------------------------------------------------------------- internal helpers
-- Not callable by any client role: EXECUTE is revoked from everyone and only the definer RPCs
-- below (which run as the owner) use them.

create or replace function public.is_room_member(p_room_id uuid, p_user uuid)
returns boolean
language sql
stable
security definer
set search_path = public, pg_temp
as $$
  select exists (select 1 from public.room_members m where m.room_id = p_room_id and m.user_id = p_user);
$$;

create or replace function public.ensure_room_owner(p_room_id uuid)
returns void
language plpgsql
security definer
set search_path = public, pg_temp
as $$
begin
  if exists (select 1 from public.room_members m where m.room_id = p_room_id and m.role = 'owner') then
    return;
  end if;
  update public.room_members set role = 'owner'
  where room_id = p_room_id
    and user_id = (select m.user_id from public.room_members m where m.room_id = p_room_id order by m.joined_at, m.user_id limit 1);
end;
$$;

-- ---------------------------------------------------------------- rooms: create, invite, join, leave, kick

create or replace function public.create_room(p_name text, p_nickname text)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_name text := btrim(coalesce(p_name, ''));
  v_nick text := btrim(coalesce(p_nickname, ''));
  v_room uuid;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if length(v_name) not between 1 and 40 or length(v_nick) not between 1 and 20 then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  if (select count(*) from public.room_members m where m.user_id = v_user and m.role = 'owner') >= 5 then
    return jsonb_build_object('code', 'room_limit');
  end if;
  insert into public.rooms (user_id, name) values (v_user, v_name) returning id into v_room;
  insert into public.room_members (room_id, user_id, role, nickname) values (v_room, v_user, 'owner', v_nick);
  return jsonb_build_object('code', 'ok', 'room_id', v_room);
end;
$$;

create or replace function public.create_invite(p_room_id uuid, p_ttl_hours integer, p_max_uses integer)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_alphabet constant text := '0123456789ABCDEFGHJKMNPQRSTVWXYZ';
  -- Bytes 6 and 8 of a version-4 UUID carry fixed version/variant bits, so they are skipped.
  v_positions constant integer[] := array[0, 1, 2, 3, 4, 5, 9, 10, 11, 12];
  v_bytes bytea;
  v_code text;
  v_pos integer;
  v_id uuid;
  v_expires timestamptz;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if p_ttl_hours is null or p_max_uses is null or p_ttl_hours not between 1 and 168 or p_max_uses not between 1 and 50 then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  if not exists (select 1 from public.room_members m where m.room_id = p_room_id and m.user_id = v_user and m.role = 'owner') then
    return jsonb_build_object('code', 'not_owner');
  end if;
  if (select count(*) from public.room_invites i
      where i.room_id = p_room_id and i.revoked_at is null and i.expires_at > now() and i.uses < i.max_uses) >= 10 then
    return jsonb_build_object('code', 'too_many_invites');
  end if;
  v_expires := now() + make_interval(hours => p_ttl_hours);
  for attempt in 1..5 loop
    v_bytes := decode(replace(gen_random_uuid()::text, '-', ''), 'hex');
    v_code := '';
    foreach v_pos in array v_positions loop
      v_code := v_code || substr(v_alphabet, (get_byte(v_bytes, v_pos) % 32) + 1, 1);
    end loop;
    begin
      insert into public.room_invites (room_id, code_hash, expires_at, max_uses)
      values (p_room_id, encode(sha256(convert_to(v_code, 'UTF8')), 'hex'), v_expires, p_max_uses)
      returning id into v_id;
      return jsonb_build_object(
        'code', 'ok', 'invite_id', v_id, 'invite_code', v_code,
        'expires_at_ms', (extract(epoch from v_expires) * 1000)::bigint
      );
    exception when unique_violation then
      null;
    end;
  end loop;
  return jsonb_build_object('code', 'invalid_input');
end;
$$;

create or replace function public.revoke_invite(p_room_id uuid, p_invite_id uuid)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_changed integer;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if not exists (select 1 from public.room_members m where m.room_id = p_room_id and m.user_id = v_user and m.role = 'owner') then
    return jsonb_build_object('code', 'not_owner');
  end if;
  update public.room_invites set revoked_at = now()
  where id = p_invite_id and room_id = p_room_id and revoked_at is null;
  get diagnostics v_changed = row_count;
  return jsonb_build_object('code', case when v_changed = 1 then 'ok' else 'not_found' end);
end;
$$;

-- Joining is only possible here: there is no INSERT grant on room_members. Every call is first
-- recorded in join_attempts (at most 5 a minute per user); because failures are returned and not
-- raised, that record survives the failure.
create or replace function public.join_room(p_code text, p_nickname text)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_nick text := btrim(coalesce(p_nickname, ''));
  v_norm text;
  v_invite public.room_invites%rowtype;
  v_capacity integer;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  delete from public.join_attempts where occurred_at < now() - interval '1 day';
  if (select count(*) from public.join_attempts a where a.user_id = v_user and a.occurred_at > now() - interval '1 minute') >= 5 then
    return jsonb_build_object('code', 'rate_limited');
  end if;
  insert into public.join_attempts (user_id) values (v_user);

  if length(v_nick) not between 1 and 20 then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  v_norm := translate(upper(regexp_replace(coalesce(p_code, ''), '[^0-9A-Za-z]', '', 'g')), 'OIL', '011');
  if v_norm !~ '^[0-9A-HJKMNP-TV-Z]{10}$' then
    return jsonb_build_object('code', 'invalid_code');
  end if;
  select * into v_invite from public.room_invites i
    where i.code_hash = encode(sha256(convert_to(v_norm, 'UTF8')), 'hex');
  if not found then
    return jsonb_build_object('code', 'invalid_code');
  end if;
  if v_invite.revoked_at is not null then
    return jsonb_build_object('code', 'revoked');
  end if;
  if v_invite.expires_at <= now() then
    return jsonb_build_object('code', 'expired');
  end if;
  if v_invite.uses >= v_invite.max_uses then
    return jsonb_build_object('code', 'exhausted');
  end if;
  if exists (select 1 from public.room_bans b where b.room_id = v_invite.room_id and b.user_id = v_user) then
    return jsonb_build_object('code', 'banned');
  end if;
  if exists (select 1 from public.room_members m where m.room_id = v_invite.room_id and m.user_id = v_user) then
    return jsonb_build_object('code', 'already_member', 'room_id', v_invite.room_id);
  end if;
  select r.capacity into v_capacity from public.rooms r where r.id = v_invite.room_id;
  if (select count(*) from public.room_members m where m.room_id = v_invite.room_id) >= v_capacity then
    return jsonb_build_object('code', 'room_full');
  end if;
  insert into public.room_members (room_id, user_id, role, nickname) values (v_invite.room_id, v_user, 'member', v_nick);
  update public.room_invites set uses = uses + 1 where id = v_invite.id;
  return jsonb_build_object('code', 'ok', 'room_id', v_invite.room_id);
end;
$$;

create or replace function public.leave_room(p_room_id uuid)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_role text;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  select m.role into v_role from public.room_members m where m.room_id = p_room_id and m.user_id = v_user;
  if not found then
    return jsonb_build_object('code', 'not_member');
  end if;
  delete from public.room_members where room_id = p_room_id and user_id = v_user;
  if not exists (select 1 from public.room_members m where m.room_id = p_room_id) then
    delete from public.rooms where id = p_room_id;
  elsif v_role = 'owner' then
    perform public.ensure_room_owner(p_room_id);
  end if;
  return jsonb_build_object('code', 'ok');
end;
$$;

create or replace function public.kick_member(p_room_id uuid, p_target_user uuid)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if not exists (select 1 from public.room_members m where m.room_id = p_room_id and m.user_id = v_user and m.role = 'owner') then
    return jsonb_build_object('code', 'not_owner');
  end if;
  if p_target_user is null or p_target_user = v_user then
    return jsonb_build_object('code', 'cannot_kick_self');
  end if;
  if not public.is_room_member(p_room_id, p_target_user) then
    return jsonb_build_object('code', 'not_member');
  end if;
  delete from public.room_members where room_id = p_room_id and user_id = p_target_user;
  insert into public.room_bans (room_id, user_id) values (p_room_id, p_target_user) on conflict do nothing;
  return jsonb_build_object('code', 'ok');
end;
$$;

-- ---------------------------------------------------------------- rooms: reading

create or replace function public.list_my_rooms()
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_room uuid;
  v_rooms jsonb;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  for v_room in select m.room_id from public.room_members m where m.user_id = v_user loop
    perform public.ensure_room_owner(v_room);
  end loop;
  select coalesce(jsonb_agg(jsonb_build_object(
           'room_id', r.id, 'name', r.name, 'role', m.role, 'capacity', r.capacity,
           'member_count', (select count(*) from public.room_members c where c.room_id = r.id)
         ) order by r.created_at, r.id), '[]'::jsonb)
    into v_rooms
    from public.room_members m join public.rooms r on r.id = m.room_id
    where m.user_id = v_user;
  return jsonb_build_object('code', 'ok', 'rooms', v_rooms);
end;
$$;

-- One call returns everything the room screen draws. Visibility is decided here, per member, from
-- friend_prefs (a member with no prefs row gets the defaults: level and status on, the rest off).
-- A hidden field is JSON null. The viewer always sees their own values. Invite rows (never the
-- code or its hash) are included for the owner only.
create or replace function public.room_snapshot(p_room_id uuid)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_role text;
  v_room public.rooms%rowtype;
  v_members jsonb;
  v_invites jsonb := '[]'::jsonb;
  v_schedule jsonb;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if not public.is_room_member(p_room_id, v_user) then
    return jsonb_build_object('code', 'not_member');
  end if;
  perform public.ensure_room_owner(p_room_id);
  select m.role into v_role from public.room_members m where m.room_id = p_room_id and m.user_id = v_user;
  select * into v_room from public.rooms r where r.id = p_room_id;

  select coalesce(jsonb_agg(jsonb_build_object(
      'user_id', m.user_id,
      'nickname', m.nickname,
      'role', m.role,
      'is_self', m.user_id = v_user,
      'level', case when coalesce(fp.share_level, true) or m.user_id = v_user then s.level end,
      'status', case when (coalesce(fp.share_status, true) or m.user_id = v_user) and s.user_id is not null
                     then jsonb_build_object('state', s.state, 'remaining_sec', s.remaining_sec,
                                             'age_sec', greatest(0, extract(epoch from now() - s.updated_at)::integer))
                end,
      'attempts_today', case when (coalesce(fp.share_attempts, false) or m.user_id = v_user) then s.attempts_today end,
      'days', case when (coalesce(fp.share_week_time, false) or m.user_id = v_user) then
                (select coalesce(jsonb_agg(jsonb_build_object('day', d.day, 'study_min', d.study_min, 'plan_min', d.plan_min) order by d.day), '[]'::jsonb)
                   from public.daily_summaries d
                  where d.user_id = m.user_id and d.day >= (now() - interval '16 days')::date)
              end,
      'character', case when (coalesce(fp.share_level, true) or m.user_id = v_user) and inv.user_id is not null
                        then jsonb_build_object('species', inv.species, 'hat', inv.slot_hat, 'top', inv.slot_top,
                                                'accessory', inv.slot_accessory, 'hand', inv.slot_hand)
                   end
    ) order by m.joined_at, m.user_id), '[]'::jsonb)
    into v_members
    from public.room_members m
    left join public.friend_prefs fp on fp.user_id = m.user_id
    left join public.member_status s on s.user_id = m.user_id
    left join public.inventory_public inv on inv.user_id = m.user_id
    where m.room_id = p_room_id;

  if v_role = 'owner' then
    select coalesce(jsonb_agg(jsonb_build_object(
             'invite_id', i.id, 'expires_at_ms', (extract(epoch from i.expires_at) * 1000)::bigint,
             'max_uses', i.max_uses, 'uses', i.uses, 'revoked', i.revoked_at is not null
           ) order by i.created_at desc), '[]'::jsonb)
      into v_invites
      from public.room_invites i where i.room_id = p_room_id;
  end if;

  select jsonb_build_object('items', rs.items, 'updated_at_ms', (extract(epoch from rs.updated_at) * 1000)::bigint)
    into v_schedule
    from public.room_schedules rs where rs.room_id = p_room_id;

  return jsonb_build_object(
    'code', 'ok',
    'server_now_ms', (extract(epoch from now()) * 1000)::bigint,
    'room', jsonb_build_object('room_id', v_room.id, 'name', v_room.name, 'capacity', v_room.capacity, 'role', v_role),
    'members', v_members,
    'invites', v_invites,
    'schedule', v_schedule
  );
end;
$$;

-- ---------------------------------------------------------------- the user's own status and summaries

create or replace function public.report_status(p_state text, p_remaining_sec integer, p_level integer, p_attempts_today integer)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_attempts integer;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if p_state is null or p_state not in ('studying', 'break', 'idle')
     or p_remaining_sec is null or p_remaining_sec not between 0 and 86400
     or p_level is null or p_level not between 1 and 999
     or (p_attempts_today is not null and p_attempts_today < 0) then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  -- The server keeps the count only for a member who chose to share it.
  select case when fp.share_attempts then p_attempts_today end into v_attempts
    from (select 1) one left join public.friend_prefs fp on fp.user_id = v_user;
  insert into public.member_status (user_id, state, remaining_sec, level, attempts_today, updated_at)
  values (v_user, p_state, p_remaining_sec, p_level, v_attempts, now())
  on conflict (user_id) do update
    set state = excluded.state, remaining_sec = excluded.remaining_sec, level = excluded.level,
        attempts_today = excluded.attempts_today, updated_at = now();
  return jsonb_build_object('code', 'ok');
end;
$$;

-- Ranking input (O-21: the client reports, the server only bounds it). A day is the member's own
-- local calendar date. Limits: at most 1440 minutes a day, a date no older than 8 days and no
-- later than tomorrow (UTC), and a rise of at most the minutes elapsed since the row was last
-- written plus 5, so a sudden jump is refused while a PC that was off can still catch up.
create or replace function public.report_daily(p_day date, p_study_min integer, p_plan_min integer)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_old public.daily_summaries%rowtype;
  v_elapsed_min integer;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if p_day is null or p_study_min is null or p_plan_min is null
     or p_study_min not between 0 and 1440 or p_plan_min not between 0 and 1440
     or p_day < (now() - interval '8 days')::date or p_day > (now() + interval '1 day')::date then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  if not coalesce((select fp.share_week_time from public.friend_prefs fp where fp.user_id = v_user), false) then
    return jsonb_build_object('code', 'not_shared');
  end if;
  select * into v_old from public.daily_summaries d where d.user_id = v_user and d.day = p_day;
  if found then
    v_elapsed_min := greatest(0, extract(epoch from now() - v_old.updated_at)::integer / 60);
    if p_study_min - v_old.study_min > v_elapsed_min + 5 then
      return jsonb_build_object('code', 'surge_rejected');
    end if;
  elsif p_study_min > 1440 then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  insert into public.daily_summaries (user_id, day, study_min, plan_min, updated_at)
  values (v_user, p_day, p_study_min, p_plan_min, now())
  on conflict (user_id, day) do update
    set study_min = excluded.study_min, plan_min = excluded.plan_min, updated_at = now();
  delete from public.daily_summaries where user_id = v_user and day < (now() - interval '60 days')::date;
  return jsonb_build_object('code', 'ok');
end;
$$;

-- Turning a switch off removes what it covered, so "stop sharing" is not only a display change.
create or replace function public.set_friend_prefs(
  p_share_level boolean,
  p_share_status boolean,
  p_share_week_time boolean,
  p_share_attempts boolean,
  p_distraction_publish boolean,
  p_distraction_receive boolean
) returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if p_share_level is null or p_share_status is null or p_share_week_time is null
     or p_share_attempts is null or p_distraction_publish is null or p_distraction_receive is null then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  insert into public.friend_prefs (user_id, share_level, share_status, share_week_time, share_attempts,
                                   distraction_publish, distraction_receive, updated_at)
  values (v_user, p_share_level, p_share_status, p_share_week_time, p_share_attempts,
          p_distraction_publish, p_distraction_receive, now())
  on conflict (user_id) do update
    set share_level = excluded.share_level, share_status = excluded.share_status,
        share_week_time = excluded.share_week_time, share_attempts = excluded.share_attempts,
        distraction_publish = excluded.distraction_publish, distraction_receive = excluded.distraction_receive,
        updated_at = now();
  if not p_share_attempts then
    update public.member_status set attempts_today = null where user_id = v_user;
  end if;
  if not p_share_week_time then
    delete from public.daily_summaries where user_id = v_user;
  end if;
  if not p_share_level and not p_share_status then
    delete from public.member_status where user_id = v_user;
  end if;
  if not p_share_level then
    delete from public.inventory_public where user_id = v_user;
  end if;
  if not p_distraction_publish then
    delete from public.distraction_events where user_id = v_user;
  end if;
  return jsonb_build_object('code', 'ok');
end;
$$;

create or replace function public.set_inventory_public(
  p_species text,
  p_hat text,
  p_top text,
  p_accessory text,
  p_hand text
) returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if p_species is null or p_species !~ '^[a-z0-9_]{1,40}$'
     or (p_hat is not null and p_hat !~ '^[a-z0-9_]{1,40}$')
     or (p_top is not null and p_top !~ '^[a-z0-9_]{1,40}$')
     or (p_accessory is not null and p_accessory !~ '^[a-z0-9_]{1,40}$')
     or (p_hand is not null and p_hand !~ '^[a-z0-9_]{1,40}$') then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  insert into public.inventory_public (user_id, species, slot_hat, slot_top, slot_accessory, slot_hand, updated_at)
  values (v_user, p_species, p_hat, p_top, p_accessory, p_hand, now())
  on conflict (user_id) do update
    set species = excluded.species, slot_hat = excluded.slot_hat, slot_top = excluded.slot_top,
        slot_accessory = excluded.slot_accessory, slot_hand = excluded.slot_hand, updated_at = now();
  return jsonb_build_object('code', 'ok');
end;
$$;

-- ---------------------------------------------------------------- distraction events (S4.6)

-- "Someone got distracted" with no content. Stored only for a member who turned publishing on.
create or replace function public.report_distraction()
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if not coalesce((select fp.distraction_publish from public.friend_prefs fp where fp.user_id = v_user), false) then
    return jsonb_build_object('code', 'disabled');
  end if;
  delete from public.distraction_events where occurred_at < now() - interval '1 day';
  if exists (select 1 from public.distraction_events e where e.user_id = v_user and e.occurred_at > now() - interval '10 seconds') then
    return jsonb_build_object('code', 'throttled');
  end if;
  insert into public.distraction_events (user_id) values (v_user);
  return jsonb_build_object('code', 'ok');
end;
$$;

-- Events reach only a receiver who turned receiving on, from a sender who turned publishing on,
-- and only between members of a room they share.
create or replace function public.poll_distraction_events(p_since_ms bigint)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_events jsonb;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if p_since_ms is null or p_since_ms < 0 then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  if not coalesce((select fp.distraction_receive from public.friend_prefs fp where fp.user_id = v_user), false) then
    return jsonb_build_object('code', 'disabled', 'events', '[]'::jsonb);
  end if;
  select coalesce(jsonb_agg(x.item order by x.at_ms), '[]'::jsonb) into v_events
  from (
    select (extract(epoch from e.occurred_at) * 1000)::bigint as at_ms,
           jsonb_build_object('event_id', e.id, 'user_id', e.user_id,
                              'nickname', (select m.nickname from public.room_members m
                                            where m.user_id = e.user_id
                                              and public.is_room_member(m.room_id, v_user)
                                            order by m.joined_at limit 1),
                              'at_ms', (extract(epoch from e.occurred_at) * 1000)::bigint) as item
    from public.distraction_events e
    join public.friend_prefs sp on sp.user_id = e.user_id and sp.distraction_publish
    where e.user_id <> v_user
      and e.occurred_at > to_timestamp(p_since_ms / 1000.0)
      and exists (select 1 from public.room_members a
                  join public.room_members b on b.room_id = a.room_id
                  where a.user_id = v_user and b.user_id = e.user_id)
    order by e.occurred_at
    limit 50
  ) x;
  return jsonb_build_object('code', 'ok', 'events', v_events);
end;
$$;

-- ---------------------------------------------------------------- same-time study (S4.5)

-- The owner publishes up to 20 upcoming blocks (start, plan name, subject, sets). Members preview
-- them and copy chosen ones into their own plan on their own PC; nothing is applied for them.
create or replace function public.share_schedule(p_room_id uuid, p_items jsonb)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_item jsonb;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if not exists (select 1 from public.room_members m where m.room_id = p_room_id and m.user_id = v_user and m.role = 'owner') then
    return jsonb_build_object('code', 'not_owner');
  end if;
  if p_items is null or jsonb_typeof(p_items) <> 'array' or jsonb_array_length(p_items) > 20 then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  begin
    for v_item in select * from jsonb_array_elements(p_items) loop
      if jsonb_typeof(v_item) <> 'object'
         or ((v_item ->> 'start_at_utc_ms')::bigint) is null
         or length(coalesce(v_item ->> 'name', '')) not between 1 and 60
         or length(coalesce(v_item ->> 'subject', '')) not between 1 and 40
         or coalesce((v_item ->> 'sets')::integer, 0) not between 1 and 12 then
        return jsonb_build_object('code', 'invalid_input');
      end if;
    end loop;
  exception when others then
    return jsonb_build_object('code', 'invalid_input');
  end;
  insert into public.room_schedules (room_id, user_id, items, updated_at)
  values (p_room_id, v_user, p_items, now())
  on conflict (room_id) do update set user_id = excluded.user_id, items = excluded.items, updated_at = now();
  return jsonb_build_object('code', 'ok');
end;
$$;

-- ---------------------------------------------------------------- function privileges

revoke execute on function public.is_room_member(uuid, uuid) from public, anon, authenticated;
revoke execute on function public.ensure_room_owner(uuid) from public, anon, authenticated;

revoke execute on function public.create_room(text, text) from public, anon;
revoke execute on function public.create_invite(uuid, integer, integer) from public, anon;
revoke execute on function public.revoke_invite(uuid, uuid) from public, anon;
revoke execute on function public.join_room(text, text) from public, anon;
revoke execute on function public.leave_room(uuid) from public, anon;
revoke execute on function public.kick_member(uuid, uuid) from public, anon;
revoke execute on function public.list_my_rooms() from public, anon;
revoke execute on function public.room_snapshot(uuid) from public, anon;
revoke execute on function public.report_status(text, integer, integer, integer) from public, anon;
revoke execute on function public.report_daily(date, integer, integer) from public, anon;
revoke execute on function public.set_friend_prefs(boolean, boolean, boolean, boolean, boolean, boolean) from public, anon;
revoke execute on function public.set_inventory_public(text, text, text, text, text) from public, anon;
revoke execute on function public.report_distraction() from public, anon;
revoke execute on function public.poll_distraction_events(bigint) from public, anon;
revoke execute on function public.share_schedule(uuid, jsonb) from public, anon;

grant execute on function public.create_room(text, text) to authenticated;
grant execute on function public.create_invite(uuid, integer, integer) to authenticated;
grant execute on function public.revoke_invite(uuid, uuid) to authenticated;
grant execute on function public.join_room(text, text) to authenticated;
grant execute on function public.leave_room(uuid) to authenticated;
grant execute on function public.kick_member(uuid, uuid) to authenticated;
grant execute on function public.list_my_rooms() to authenticated;
grant execute on function public.room_snapshot(uuid) to authenticated;
grant execute on function public.report_status(text, integer, integer, integer) to authenticated;
grant execute on function public.report_daily(date, integer, integer) to authenticated;
grant execute on function public.set_friend_prefs(boolean, boolean, boolean, boolean, boolean, boolean) to authenticated;
grant execute on function public.set_inventory_public(text, text, text, text, text) to authenticated;
grant execute on function public.report_distraction() to authenticated;
grant execute on function public.poll_distraction_events(bigint) to authenticated;
grant execute on function public.share_schedule(uuid, jsonb) to authenticated;
