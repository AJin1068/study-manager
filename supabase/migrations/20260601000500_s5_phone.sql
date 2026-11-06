-- Stage 5 (S5.2 / S5.3 / S5.5): pairing a phone with the desktop app, the lock lease the desktop
-- publishes, the phone's heartbeat and its pick-up counts.
--
-- Who is who
--   * The OWNER is the signed-in desktop user. The PHONE signs in anonymously (Supabase anonymous
--     sign-in gives it its own auth.uid() with the `authenticated` role) and is bound to exactly one
--     owner by claim_pair(code). After that it may read that owner's phone_* rows and nothing else
--     of that owner's data.
--   * Pairing code: made by the owner (create_pair_code), 10 characters of the 32-letter Crockford
--     alphabet from the server CSPRNG, valid for 5 minutes, usable once. Only sha256(code) is
--     stored. The QR carries the code, the server address and the PUBLIC anon key; no long-lived
--     secret.
--   * Revoking (revoke_pair, by either side) sets revoked_at. Every policy below tests
--     revoked_at IS NULL, so access ends at once even though the phone's JWT stays valid.
--   * Clients can only SELECT the phone_* tables (policies below). All writes are definer RPCs.
--     pair_codes and pair_attempts are not readable or writable by any client role.
--   * Refusals come back as {"code": ...}, never RAISE, so the attempt counter written by
--     claim_pair is not rolled back by a failing call.
--   * The lease (phone_lock_state) is an instruction with an expiry, not a connection: the phone
--     obeys it offline until valid_until_ms and then frees itself, so a dead PC or server cannot
--     lock a phone forever. The server refuses a lease longer than one segment cap (3 hours) plus
--     the 60 s grace, and refuses a sequence number that does not rise.
--   * The phone reports counts and ids only: no app names, no usage history.

-- ---------------------------------------------------------------- tables

create table public.phone_pairs (
  id            uuid primary key default gen_random_uuid(),
  owner_user_id uuid not null references auth.users (id) on delete cascade,
  phone_user_id uuid not null references auth.users (id) on delete cascade,
  paired_at     timestamptz not null default now(),
  revoked_at    timestamptz,
  check (owner_user_id <> phone_user_id)
);
-- One active phone per owner and one active owner per phone.
create unique index phone_pairs_one_owner on public.phone_pairs (owner_user_id) where revoked_at is null;
create unique index phone_pairs_one_phone on public.phone_pairs (phone_user_id) where revoked_at is null;

create table public.pair_codes (
  id            uuid primary key default gen_random_uuid(),
  owner_user_id uuid not null references auth.users (id) on delete cascade,
  code_hash     text not null unique,
  expires_at    timestamptz not null,
  used_at       timestamptz,
  created_at    timestamptz not null default now()
);

create table public.pair_attempts (
  id          bigint generated always as identity primary key,
  user_id     uuid not null references auth.users (id) on delete cascade,
  occurred_at timestamptz not null default now()
);
create index pair_attempts_user on public.pair_attempts (user_id, occurred_at);

-- The latest lease of one owner. seq only rises.
create table public.phone_lock_state (
  owner_user_id    uuid primary key references auth.users (id) on delete cascade,
  seq              bigint not null check (seq >= 1),
  run_id           text not null check (length(run_id) between 1 and 64),
  state            text not null check (state in ('locked', 'free')),
  release_cause    text check (release_cause in ('run_ended', 'emergency_pass', 'skip_today', 'early_release', 'pair_revoked')),
  issued_at_ms     bigint not null,
  valid_until_ms   bigint not null,
  updated_at       timestamptz not null default now(),
  check ((state = 'locked' and release_cause is null and valid_until_ms > issued_at_ms and valid_until_ms - issued_at_ms <= 10860000)
      or (state = 'free' and release_cause is not null and valid_until_ms = issued_at_ms))
);

create table public.phone_beats (
  id              bigint generated always as identity primary key,
  owner_user_id   uuid not null references auth.users (id) on delete cascade,
  phone_user_id   uuid not null references auth.users (id) on delete cascade,
  lease_seq       bigint not null check (lease_seq >= 0),
  locked          boolean not null,
  service_enabled boolean not null,
  at              timestamptz not null default now()
);
create index phone_beats_owner on public.phone_beats (owner_user_id, at);

-- Cumulative pick-up count of one study segment as the phone saw it. A report is appended; the same
-- (segment, count) again is ignored.
create table public.phone_pickups (
  id            bigint generated always as identity primary key,
  owner_user_id uuid not null references auth.users (id) on delete cascade,
  phone_user_id uuid not null references auth.users (id) on delete cascade,
  segment_id    text not null check (length(segment_id) between 1 and 64),
  count         integer not null check (count between 0 and 100000),
  reported_at   timestamptz not null default now(),
  unique (owner_user_id, segment_id, count)
);

alter table public.phone_pairs      enable row level security;
alter table public.pair_codes       enable row level security;
alter table public.pair_attempts    enable row level security;
alter table public.phone_lock_state enable row level security;
alter table public.phone_beats      enable row level security;
alter table public.phone_pickups    enable row level security;

-- Table privileges mirror fixtures/phone/rls-matrix.json: SELECT on the four phone tables for
-- authenticated (further narrowed by the policies), nothing at all for anon, nothing on the
-- code and attempt tables.
revoke all on public.phone_pairs, public.pair_codes, public.pair_attempts, public.phone_lock_state,
  public.phone_beats, public.phone_pickups
  from public, anon, authenticated;
grant select on public.phone_pairs to authenticated;
grant select on public.phone_lock_state to authenticated;
grant select on public.phone_beats to authenticated;
grant select on public.phone_pickups to authenticated;

create policy phone_pairs_select on public.phone_pairs
  for select to authenticated
  using (owner_user_id = auth.uid() or phone_user_id = auth.uid());

create policy phone_lock_state_select on public.phone_lock_state
  for select to authenticated
  using (
    owner_user_id = auth.uid()
    or exists (
      select 1 from public.phone_pairs p
      where p.owner_user_id = phone_lock_state.owner_user_id and p.phone_user_id = auth.uid() and p.revoked_at is null
    )
  );

create policy phone_beats_select on public.phone_beats
  for select to authenticated
  using (
    owner_user_id = auth.uid()
    or exists (
      select 1 from public.phone_pairs p
      where p.owner_user_id = phone_beats.owner_user_id and p.phone_user_id = auth.uid() and p.revoked_at is null
    )
  );

create policy phone_pickups_select on public.phone_pickups
  for select to authenticated
  using (
    owner_user_id = auth.uid()
    or exists (
      select 1 from public.phone_pairs p
      where p.owner_user_id = phone_pickups.owner_user_id and p.phone_user_id = auth.uid() and p.revoked_at is null
    )
  );

-- ---------------------------------------------------------------- internal helper
-- Not callable by any client role.

create or replace function public.phone_owner_of(p_phone uuid)
returns uuid
language sql
stable
security definer
set search_path = public, pg_temp
as $$
  select p.owner_user_id from public.phone_pairs p where p.phone_user_id = p_phone and p.revoked_at is null;
$$;

-- ---------------------------------------------------------------- pairing

create or replace function public.create_pair_code()
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
  v_expires timestamptz;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  -- A phone (anonymous account) cannot make codes: only a real account owns a study plan.
  if coalesce((auth.jwt() ->> 'is_anonymous')::boolean, false) then
    return jsonb_build_object('code', 'anonymous_not_allowed');
  end if;
  if exists (select 1 from public.phone_pairs p where p.owner_user_id = v_user and p.revoked_at is null) then
    return jsonb_build_object('code', 'already_paired');
  end if;
  -- A new code retires the earlier unused ones: at most one live code per owner.
  update public.pair_codes set expires_at = now()
  where owner_user_id = v_user and used_at is null and expires_at > now();
  v_expires := now() + interval '5 minutes';
  for attempt in 1..5 loop
    v_bytes := decode(replace(gen_random_uuid()::text, '-', ''), 'hex');
    v_code := '';
    foreach v_pos in array v_positions loop
      v_code := v_code || substr(v_alphabet, (get_byte(v_bytes, v_pos) % 32) + 1, 1);
    end loop;
    begin
      insert into public.pair_codes (owner_user_id, code_hash, expires_at)
      values (v_user, encode(sha256(convert_to(v_code, 'UTF8')), 'hex'), v_expires);
      return jsonb_build_object(
        'code', 'ok', 'pair_code', v_code,
        'expires_at_ms', (extract(epoch from v_expires) * 1000)::bigint
      );
    exception when unique_violation then
      null;
    end;
  end loop;
  return jsonb_build_object('code', 'invalid_input');
end;
$$;

-- The attempt row is written before anything is checked and every refusal is a returned code, so a
-- wrong guess is counted even though the call "fails".
create or replace function public.claim_pair(p_code text)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_norm text;
  v_row public.pair_codes%rowtype;
  v_pair uuid;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  delete from public.pair_attempts where occurred_at < now() - interval '1 day';
  if (select count(*) from public.pair_attempts a where a.user_id = v_user and a.occurred_at > now() - interval '1 minute') >= 5 then
    return jsonb_build_object('code', 'rate_limited');
  end if;
  insert into public.pair_attempts (user_id) values (v_user);

  v_norm := translate(upper(regexp_replace(coalesce(p_code, ''), '[^0-9A-Za-z]', '', 'g')), 'OIL', '011');
  if v_norm !~ '^[0-9A-HJKMNP-TV-Z]{10}$' then
    return jsonb_build_object('code', 'invalid_code');
  end if;
  select * into v_row from public.pair_codes c
    where c.code_hash = encode(sha256(convert_to(v_norm, 'UTF8')), 'hex')
    for update;
  if not found then
    return jsonb_build_object('code', 'invalid_code');
  end if;
  if v_row.used_at is not null then
    return jsonb_build_object('code', 'used');
  end if;
  if v_row.expires_at <= now() then
    return jsonb_build_object('code', 'expired');
  end if;
  if v_row.owner_user_id = v_user then
    return jsonb_build_object('code', 'cannot_self');
  end if;
  if exists (select 1 from public.phone_pairs p where p.owner_user_id = v_row.owner_user_id and p.revoked_at is null) then
    return jsonb_build_object('code', 'already_paired');
  end if;
  if exists (select 1 from public.phone_pairs p where p.phone_user_id = v_user and p.revoked_at is null) then
    return jsonb_build_object('code', 'phone_already_paired');
  end if;
  update public.pair_codes set used_at = now() where id = v_row.id;
  insert into public.phone_pairs (owner_user_id, phone_user_id) values (v_row.owner_user_id, v_user)
  returning id into v_pair;
  return jsonb_build_object(
    'code', 'ok', 'pair_id', v_pair, 'owner_user_id', v_row.owner_user_id,
    'server_now_ms', (extract(epoch from now()) * 1000)::bigint
  );
end;
$$;

-- Either side can end the pairing it belongs to.
create or replace function public.revoke_pair()
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
  update public.phone_pairs set revoked_at = now()
  where revoked_at is null and (owner_user_id = v_user or phone_user_id = v_user);
  get diagnostics v_changed = row_count;
  return jsonb_build_object('code', case when v_changed >= 1 then 'ok' else 'not_found' end);
end;
$$;

-- The owner's view of the pairing, for the settings screen.
create or replace function public.phone_status()
returns jsonb
language plpgsql
stable
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_pair public.phone_pairs%rowtype;
  v_beat public.phone_beats%rowtype;
  v_seq bigint;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  select * into v_pair from public.phone_pairs p where p.owner_user_id = v_user and p.revoked_at is null;
  select s.seq into v_seq from public.phone_lock_state s where s.owner_user_id = v_user;
  if not found then
    v_seq := 0;
  end if;
  if v_pair.id is null then
    return jsonb_build_object('code', 'ok', 'paired', false, 'lease_seq', v_seq);
  end if;
  select * into v_beat from public.phone_beats b where b.owner_user_id = v_user order by b.at desc, b.id desc limit 1;
  return jsonb_build_object(
    'code', 'ok', 'paired', true,
    'paired_at_ms', (extract(epoch from v_pair.paired_at) * 1000)::bigint,
    'lease_seq', v_seq,
    'last_beat_ms', case when v_beat.id is null then null else (extract(epoch from v_beat.at) * 1000)::bigint end,
    'service_enabled', case when v_beat.id is null then null else v_beat.service_enabled end,
    'server_now_ms', (extract(epoch from now()) * 1000)::bigint
  );
end;
$$;

-- ---------------------------------------------------------------- the lease

-- The owner publishes the lock state. `p_seq` must rise; the same seq with the same content is
-- answered as a success without a second write (a retry after a lost answer), any other content
-- under a used seq is stale_seq and carries the current seq so the desktop can catch up.
create or replace function public.publish_phone_lock(p_seq bigint, p_run_id text, p_state text, p_release_cause text, p_valid_until_ms bigint)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_now bigint := (extract(epoch from now()) * 1000)::bigint;
  v_cur public.phone_lock_state%rowtype;
  v_valid bigint;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if p_seq is null or p_seq < 1 or p_run_id is null or length(p_run_id) not between 1 and 64
     or p_state is null or p_state not in ('locked', 'free') then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  if p_state = 'locked' then
    if p_release_cause is not null or p_valid_until_ms is null or p_valid_until_ms <= v_now then
      return jsonb_build_object('code', 'invalid_input');
    end if;
    if p_valid_until_ms - v_now > 10860000 then
      return jsonb_build_object('code', 'lease_too_long');
    end if;
    v_valid := p_valid_until_ms;
  else
    if p_release_cause is null or p_release_cause not in ('run_ended', 'emergency_pass', 'skip_today', 'early_release', 'pair_revoked') then
      return jsonb_build_object('code', 'invalid_input');
    end if;
    v_valid := v_now;
  end if;
  select * into v_cur from public.phone_lock_state s where s.owner_user_id = v_user for update;
  if found then
    if p_seq < v_cur.seq then
      return jsonb_build_object('code', 'stale_seq', 'current_seq', v_cur.seq);
    end if;
    if p_seq = v_cur.seq then
      if v_cur.run_id = p_run_id and v_cur.state = p_state and v_cur.release_cause is not distinct from p_release_cause
         and (p_state = 'free' or v_cur.valid_until_ms = p_valid_until_ms) then
        return jsonb_build_object('code', 'ok', 'seq', v_cur.seq, 'replayed', true, 'server_now_ms', v_now);
      end if;
      return jsonb_build_object('code', 'stale_seq', 'current_seq', v_cur.seq);
    end if;
    update public.phone_lock_state
    set seq = p_seq, run_id = p_run_id, state = p_state, release_cause = p_release_cause,
        issued_at_ms = v_now, valid_until_ms = v_valid, updated_at = now()
    where owner_user_id = v_user;
  else
    insert into public.phone_lock_state (owner_user_id, seq, run_id, state, release_cause, issued_at_ms, valid_until_ms)
    values (v_user, p_seq, p_run_id, p_state, p_release_cause, v_now, v_valid);
  end if;
  return jsonb_build_object('code', 'ok', 'seq', p_seq, 'replayed', false, 'server_now_ms', v_now);
end;
$$;

-- The phone reads the lease of the owner it is paired with, together with the server's clock
-- reading, which is what the monotonic-elapsed rule on the phone is anchored to.
create or replace function public.phone_lease()
returns jsonb
language plpgsql
stable
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_owner uuid;
  v_lease public.phone_lock_state%rowtype;
  v_now bigint := (extract(epoch from now()) * 1000)::bigint;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  v_owner := public.phone_owner_of(v_user);
  if v_owner is null then
    return jsonb_build_object('code', 'not_paired');
  end if;
  select * into v_lease from public.phone_lock_state s where s.owner_user_id = v_owner;
  if not found then
    return jsonb_build_object('code', 'ok', 'lease', null, 'server_now_ms', v_now);
  end if;
  return jsonb_build_object(
    'code', 'ok', 'server_now_ms', v_now,
    'lease', jsonb_build_object(
      'run_id', v_lease.run_id, 'seq', v_lease.seq, 'state', v_lease.state,
      'release_cause', v_lease.release_cause, 'issued_at_ms', v_lease.issued_at_ms,
      'valid_until_ms', v_lease.valid_until_ms
    )
  );
end;
$$;

create or replace function public.phone_beat(p_lease_seq bigint, p_locked boolean, p_service_enabled boolean)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_owner uuid;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  v_owner := public.phone_owner_of(v_user);
  if v_owner is null then
    return jsonb_build_object('code', 'not_paired');
  end if;
  if p_lease_seq is null or p_lease_seq < 0 or p_locked is null or p_service_enabled is null then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  delete from public.phone_beats where owner_user_id = v_owner and at < now() - interval '7 days';
  insert into public.phone_beats (owner_user_id, phone_user_id, lease_seq, locked, service_enabled)
  values (v_owner, v_user, p_lease_seq, p_locked, p_service_enabled);
  return jsonb_build_object('code', 'ok', 'server_now_ms', (extract(epoch from now()) * 1000)::bigint);
end;
$$;

create or replace function public.report_phone_pickups(p_segment_id text, p_count integer)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_owner uuid;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  v_owner := public.phone_owner_of(v_user);
  if v_owner is null then
    return jsonb_build_object('code', 'not_paired');
  end if;
  if p_segment_id is null or length(p_segment_id) not between 1 and 64 or p_count is null or p_count not between 0 and 100000 then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  insert into public.phone_pickups (owner_user_id, phone_user_id, segment_id, count)
  values (v_owner, v_user, p_segment_id, p_count)
  on conflict (owner_user_id, segment_id, count) do nothing;
  return jsonb_build_object('code', 'ok');
end;
$$;

-- ---------------------------------------------------------------- execute rights

revoke execute on function public.phone_owner_of(uuid) from public, anon, authenticated;

revoke execute on function public.create_pair_code() from public, anon;
revoke execute on function public.claim_pair(text) from public, anon;
revoke execute on function public.revoke_pair() from public, anon;
revoke execute on function public.phone_status() from public, anon;
revoke execute on function public.publish_phone_lock(bigint, text, text, text, bigint) from public, anon;
revoke execute on function public.phone_lease() from public, anon;
revoke execute on function public.phone_beat(bigint, boolean, boolean) from public, anon;
revoke execute on function public.report_phone_pickups(text, integer) from public, anon;

grant execute on function public.create_pair_code() to authenticated;
grant execute on function public.claim_pair(text) to authenticated;
grant execute on function public.revoke_pair() to authenticated;
grant execute on function public.phone_status() to authenticated;
grant execute on function public.publish_phone_lock(bigint, text, text, text, bigint) to authenticated;
grant execute on function public.phone_lease() to authenticated;
grant execute on function public.phone_beat(bigint, boolean, boolean) to authenticated;
grant execute on function public.report_phone_pickups(text, integer) to authenticated;
