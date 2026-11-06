-- Stage 4 (S4.9 / S4.10 / S4.11): profile home, guestbook, visitor counts, gifts, and the read
-- path of the static web bundle (web/).
--
-- Who may see what
--   * Each profile section has its own scope: public (anyone holding the profile link, including
--     anonymous callers), friends (members of a common room, O-11) or private (only the owner).
--     The default of every section is private. The scope is applied on the server inside
--     profile_view(); a section the viewer may not see is simply absent from the answer.
--   * Clients can NOT read any of these tables. Every table has row level security switched on,
--     all table privileges are revoked from public, anon and authenticated, and there is no
--     policy. All reads and writes go through the RPCs below.
--   * The only function anonymous callers may execute is get_public_profile(slug), and it returns
--     only what the owner marked public. Every other RPC is revoked from public and anon.
--   * Deviation statistics have no column here and are never part of a profile.
--   * A section switched back to private deletes what it covered (grass rows, wiki titles, the
--     current subject, level, badges, furniture), so "stop sharing" is not only a display change.
--   * Visitors are signed-in non-owners, counted once per day per person. Anonymous views are not
--     counted (they cannot be told apart and would be trivially inflated).
--   * Gifts spend the sender's own local points (never transferred), only between members of a
--     common room, only for items listed in gift_catalog (shop clothes and furniture; achievement
--     items and characters are absent), at most 5 a day per sender, and one idempotency key is
--     processed once.
--
-- Conventions follow the stage-4 room migration: security definer + pinned search_path, EXECUTE
-- revoked from PUBLIC and anon then granted explicitly, auth.uid() identifies the caller, and
-- refusals come back as {"code": "..."} (never RAISE).

-- ---------------------------------------------------------------- tables

create table public.profile_settings (
  user_id       uuid primary key default auth.uid() references auth.users (id) on delete cascade,
  slug          text not null unique check (slug ~ '^[a-f0-9]{16}$'),
  display_name  text not null check (length(display_name) between 1 and 20),
  vis_room      text not null default 'private' check (vis_room in ('public', 'friends', 'private')),
  vis_grass     text not null default 'private' check (vis_grass in ('public', 'friends', 'private')),
  vis_level     text not null default 'private' check (vis_level in ('public', 'friends', 'private')),
  vis_badges    text not null default 'private' check (vis_badges in ('public', 'friends', 'private')),
  vis_guestbook text not null default 'private' check (vis_guestbook in ('public', 'friends', 'private')),
  vis_visitors  text not null default 'private' check (vis_visitors in ('public', 'friends', 'private')),
  vis_wiki      text not null default 'private' check (vis_wiki in ('public', 'friends', 'private')),
  vis_subject   text not null default 'private' check (vis_subject in ('public', 'friends', 'private')),
  vis_gifts     text not null default 'private' check (vis_gifts in ('public', 'friends', 'private')),
  level         integer check (level is null or level between 1 and 999),
  badges        text[] not null default '{}' check (badges <@ array['streak_7', 'quiz_100']::text[]),
  furniture     text[] not null default '{}' check (cardinality(furniture) <= 12),
  wiki_titles   text[] not null default '{}' check (cardinality(wiki_titles) <= 10),
  subject_now   text check (subject_now is null or length(subject_now) between 1 and 40),
  updated_at    timestamptz not null default now()
);

create table public.profile_grass (
  user_id   uuid not null default auth.uid() references auth.users (id) on delete cascade,
  day       date not null,
  study_min integer not null check (study_min between 0 and 1440),
  primary key (user_id, day)
);

create table public.profile_guestbook (
  id          bigint generated always as identity primary key,
  owner_id    uuid not null references auth.users (id) on delete cascade,
  author_id   uuid not null references auth.users (id) on delete cascade,
  author_name text not null check (length(author_name) between 1 and 20),
  body        text not null check (length(body) between 1 and 200),
  reported    boolean not null default false,
  created_at  timestamptz not null default now()
);
create index profile_guestbook_owner on public.profile_guestbook (owner_id, created_at);
create index profile_guestbook_author on public.profile_guestbook (author_id, created_at);

create table public.profile_visits (
  owner_id   uuid not null references auth.users (id) on delete cascade,
  visitor_id uuid not null references auth.users (id) on delete cascade,
  day        date not null,
  primary key (owner_id, visitor_id, day)
);

-- Items that may be given: shop clothes and furniture. Achievement and starter items and extra
-- characters are deliberately absent. A drift test compares this list with the app's item catalog.
create table public.gift_catalog (
  item_id text primary key check (item_id ~ '^[a-z0-9_]{1,40}$')
);

create table public.gifts (
  id           uuid primary key default gen_random_uuid(),
  idem_key     uuid not null,
  sender_id    uuid not null references auth.users (id) on delete cascade,
  recipient_id uuid not null references auth.users (id) on delete cascade,
  sender_name  text not null check (length(sender_name) between 1 and 20),
  item_id      text not null references public.gift_catalog (item_id),
  message      text not null default '' check (length(message) <= 60),
  status       text not null default 'pending' check (status in ('pending', 'accepted', 'declined')),
  created_at   timestamptz not null default now(),
  resolved_at  timestamptz,
  unique (sender_id, idem_key)
);
create index gifts_recipient on public.gifts (recipient_id, created_at);
create index gifts_sender_at on public.gifts (sender_id, created_at);

insert into public.gift_catalog (item_id) values
  ('hat_beanie'), ('hat_straw'), ('hat_ribbon'),
  ('top_stripe'), ('top_overalls'), ('top_raincoat'),
  ('acc_bowtie'), ('acc_glasses'), ('acc_backpack'),
  ('hand_umbrella'), ('hand_lantern'), ('hand_mug'),
  ('furn_desk'), ('furn_lamp'), ('furn_rug'), ('furn_shelf');

-- ---------------------------------------------------------------- row level security

alter table public.profile_settings  enable row level security;
alter table public.profile_grass     enable row level security;
alter table public.profile_guestbook enable row level security;
alter table public.profile_visits    enable row level security;
alter table public.gift_catalog      enable row level security;
alter table public.gifts             enable row level security;

-- Table privileges mirror fixtures/profile/rls-matrix.json: nobody reads or writes these tables
-- through the data API, anonymous callers included.
revoke all on public.profile_settings, public.profile_grass, public.profile_guestbook,
  public.profile_visits, public.gift_catalog, public.gifts
  from public, anon, authenticated;

-- ---------------------------------------------------------------- internal helpers
-- Not callable by any client role: EXECUTE is revoked from everyone and only the definer RPCs
-- (which run as the owner) call them.

create or replace function public.profiles_share_room(p_a uuid, p_b uuid)
returns boolean
language sql
stable
security definer
set search_path = public, pg_temp
as $$
  select exists (
    select 1 from public.room_members x
    join public.room_members y on y.room_id = x.room_id
    where x.user_id = p_a and y.user_id = p_b
  );
$$;

-- public = anyone (a null viewer is an anonymous caller), friends = a common room, private = owner.
create or replace function public.profile_scope_allows(p_scope text, p_owner uuid, p_viewer uuid)
returns boolean
language sql
stable
security definer
set search_path = public, pg_temp
as $$
  select case
    when p_viewer is not null and p_viewer = p_owner then true
    when p_scope = 'public' then true
    when p_scope = 'friends' then p_viewer is not null and public.profiles_share_room(p_owner, p_viewer)
    else false
  end;
$$;

-- The one place a profile answer is assembled. A section the viewer may not see is not in the
-- answer at all. Returns null when the owner has no profile.
create or replace function public.profile_view(p_owner uuid, p_viewer uuid)
returns jsonb
language plpgsql
stable
security definer
set search_path = public, pg_temp
as $$
declare
  s public.profile_settings%rowtype;
  v_sections jsonb := '{}'::jsonb;
  v_owner boolean := p_viewer is not null and p_viewer = p_owner;
begin
  select * into s from public.profile_settings where user_id = p_owner;
  if not found then
    return null;
  end if;
  if public.profile_scope_allows(s.vis_room, p_owner, p_viewer) then
    v_sections := v_sections || jsonb_build_object('room', (
      select jsonb_build_object(
               'species', i.species, 'hat', i.slot_hat, 'top', i.slot_top,
               'accessory', i.slot_accessory, 'hand', i.slot_hand, 'furniture', to_jsonb(s.furniture))
      from (select 1) one left join public.inventory_public i on i.user_id = p_owner));
  end if;
  if public.profile_scope_allows(s.vis_grass, p_owner, p_viewer) then
    v_sections := v_sections || jsonb_build_object('grass', coalesce((
      select jsonb_agg(jsonb_build_object('day', to_char(g.day, 'YYYY-MM-DD'), 'study_min', g.study_min) order by g.day)
      from public.profile_grass g
      where g.user_id = p_owner and g.day > (now() - interval '366 days')::date), '[]'::jsonb));
  end if;
  if public.profile_scope_allows(s.vis_level, p_owner, p_viewer) then
    v_sections := v_sections || jsonb_build_object('level', s.level);
  end if;
  if public.profile_scope_allows(s.vis_badges, p_owner, p_viewer) then
    v_sections := v_sections || jsonb_build_object('badges', to_jsonb(s.badges));
  end if;
  if public.profile_scope_allows(s.vis_guestbook, p_owner, p_viewer) then
    v_sections := v_sections || jsonb_build_object('guestbook', coalesce((
      select jsonb_agg(x.item order by x.at_ms desc)
      from (
        select (extract(epoch from b.created_at) * 1000)::bigint as at_ms,
               jsonb_build_object('id', b.id, 'author_name', b.author_name, 'body', b.body,
                                  'at_ms', (extract(epoch from b.created_at) * 1000)::bigint,
                                  'mine', p_viewer is not null and b.author_id = p_viewer,
                                  'reported', case when v_owner then b.reported else false end) as item
        from public.profile_guestbook b
        where b.owner_id = p_owner and (v_owner or not b.reported)
        order by b.created_at desc
        limit 50
      ) x), '[]'::jsonb));
  end if;
  if public.profile_scope_allows(s.vis_visitors, p_owner, p_viewer) then
    v_sections := v_sections || jsonb_build_object('visitors', jsonb_build_object(
      'total', (select count(*) from public.profile_visits v where v.owner_id = p_owner),
      'today', (select count(*) from public.profile_visits v where v.owner_id = p_owner and v.day = (now() at time zone 'utc')::date)));
  end if;
  if public.profile_scope_allows(s.vis_wiki, p_owner, p_viewer) then
    v_sections := v_sections || jsonb_build_object('wiki', to_jsonb(s.wiki_titles));
  end if;
  if public.profile_scope_allows(s.vis_subject, p_owner, p_viewer) then
    v_sections := v_sections || jsonb_build_object('subject', s.subject_now);
  end if;
  if public.profile_scope_allows(s.vis_gifts, p_owner, p_viewer) then
    v_sections := v_sections || jsonb_build_object('gifts', coalesce((
      select jsonb_agg(x.item order by x.at_ms desc)
      from (
        select (extract(epoch from g.resolved_at) * 1000)::bigint as at_ms,
               jsonb_build_object('item_id', g.item_id, 'sender_name', g.sender_name, 'message', g.message,
                                  'at_ms', (extract(epoch from g.resolved_at) * 1000)::bigint) as item
        from public.gifts g
        where g.recipient_id = p_owner and g.status = 'accepted'
        order by g.resolved_at desc
        limit 20
      ) x), '[]'::jsonb));
  end if;
  return jsonb_build_object('name', s.display_name, 'sections', v_sections);
end;
$$;

-- ---------------------------------------------------------------- profile settings and reports

create or replace function public.set_profile_settings(
  p_display_name text,
  p_vis_room text,
  p_vis_grass text,
  p_vis_level text,
  p_vis_badges text,
  p_vis_guestbook text,
  p_vis_visitors text,
  p_vis_wiki text,
  p_vis_subject text,
  p_vis_gifts text,
  p_level integer,
  p_badges text[],
  p_furniture text[],
  p_wiki_titles text[],
  p_subject_now text
) returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_name text := btrim(coalesce(p_display_name, ''));
  v_scopes text[] := array[p_vis_room, p_vis_grass, p_vis_level, p_vis_badges, p_vis_guestbook,
                           p_vis_visitors, p_vis_wiki, p_vis_subject, p_vis_gifts];
  v_scope text;
  v_titles text[];
  v_subject text := nullif(btrim(coalesce(p_subject_now, '')), '');
  v_furniture text[] := coalesce(p_furniture, '{}');
  v_badges text[] := coalesce(p_badges, '{}');
  v_slug text;
  v_try integer;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if length(v_name) not between 1 and 20 then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  foreach v_scope in array v_scopes loop
    if v_scope is null or v_scope not in ('public', 'friends', 'private') then
      return jsonb_build_object('code', 'invalid_input');
    end if;
  end loop;
  if p_level is not null and p_level not between 1 and 999 then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  if cardinality(v_furniture) > 12
     or exists (select 1 from unnest(v_furniture) f where f is null or f !~ '^[a-z0-9_]{1,40}$') then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  if not (v_badges <@ array['streak_7', 'quiz_100']::text[]) then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  select coalesce(array_agg(btrim(t)), '{}') into v_titles from unnest(coalesce(p_wiki_titles, '{}')) t;
  if cardinality(v_titles) > 10 or exists (select 1 from unnest(v_titles) t where t is null or length(t) not between 1 and 60) then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  if v_subject is not null and length(v_subject) > 40 then
    return jsonb_build_object('code', 'invalid_input');
  end if;

  -- A section kept private stores nothing.
  if p_vis_room = 'private' then v_furniture := '{}'; end if;
  if p_vis_badges = 'private' then v_badges := '{}'; end if;
  if p_vis_wiki = 'private' then v_titles := '{}'; end if;
  if p_vis_subject = 'private' then v_subject := null; end if;
  if p_vis_grass = 'private' then
    delete from public.profile_grass where user_id = v_user;
  end if;

  if exists (select 1 from public.profile_settings where user_id = v_user) then
    update public.profile_settings set
      display_name = v_name, vis_room = p_vis_room, vis_grass = p_vis_grass, vis_level = p_vis_level,
      vis_badges = p_vis_badges, vis_guestbook = p_vis_guestbook, vis_visitors = p_vis_visitors,
      vis_wiki = p_vis_wiki, vis_subject = p_vis_subject, vis_gifts = p_vis_gifts,
      level = case when p_vis_level = 'private' then null else p_level end,
      badges = v_badges, furniture = v_furniture, wiki_titles = v_titles, subject_now = v_subject,
      updated_at = now()
    where user_id = v_user
    returning slug into v_slug;
  else
    for v_try in 1..5 loop
      begin
        insert into public.profile_settings (
          user_id, slug, display_name, vis_room, vis_grass, vis_level, vis_badges, vis_guestbook,
          vis_visitors, vis_wiki, vis_subject, vis_gifts, level, badges, furniture, wiki_titles, subject_now)
        values (
          v_user, substr(replace(gen_random_uuid()::text, '-', ''), 1, 16), v_name, p_vis_room, p_vis_grass,
          p_vis_level, p_vis_badges, p_vis_guestbook, p_vis_visitors, p_vis_wiki, p_vis_subject, p_vis_gifts,
          case when p_vis_level = 'private' then null else p_level end, v_badges, v_furniture, v_titles, v_subject)
        returning slug into v_slug;
        exit;
      exception when unique_violation then
        v_slug := null;
      end;
    end loop;
    if v_slug is null then
      return jsonb_build_object('code', 'invalid_input');
    end if;
  end if;
  return jsonb_build_object('code', 'ok', 'slug', v_slug);
end;
$$;

-- Daily study minutes for the year grass: up to 31 entries {day, study_min} per call, days within
-- the last 366 days (and not later than tomorrow, UTC). Refused while the grass is private.
create or replace function public.report_profile_grass(p_entries jsonb)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_item jsonb;
  v_day date;
  v_min integer;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if p_entries is null or jsonb_typeof(p_entries) <> 'array' or jsonb_array_length(p_entries) > 31 then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  if not exists (select 1 from public.profile_settings s where s.user_id = v_user and s.vis_grass <> 'private') then
    return jsonb_build_object('code', 'not_shared');
  end if;
  begin
    for v_item in select * from jsonb_array_elements(p_entries) loop
      v_day := (v_item ->> 'day')::date;
      v_min := (v_item ->> 'study_min')::integer;
      if v_day is null or v_min is null or v_min not between 0 and 1440
         or v_day < (now() - interval '366 days')::date or v_day > (now() + interval '1 day')::date then
        return jsonb_build_object('code', 'invalid_input');
      end if;
    end loop;
  exception when others then
    return jsonb_build_object('code', 'invalid_input');
  end;
  for v_item in select * from jsonb_array_elements(p_entries) loop
    insert into public.profile_grass (user_id, day, study_min)
    values (v_user, (v_item ->> 'day')::date, (v_item ->> 'study_min')::integer)
    on conflict (user_id, day) do update set study_min = excluded.study_min;
  end loop;
  delete from public.profile_grass where user_id = v_user and day <= (now() - interval '366 days')::date;
  return jsonb_build_object('code', 'ok');
end;
$$;

-- ---------------------------------------------------------------- reading profiles

-- The caller's own profile with every section, plus the slug and the scopes the app edits.
create or replace function public.get_my_profile()
returns jsonb
language plpgsql
stable
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_view jsonb;
  s public.profile_settings%rowtype;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  v_view := public.profile_view(v_user, v_user);
  if v_view is null then
    return jsonb_build_object('code', 'not_found');
  end if;
  select * into s from public.profile_settings where user_id = v_user;
  return jsonb_build_object('code', 'ok', 'slug', s.slug, 'name', v_view -> 'name', 'sections', v_view -> 'sections');
end;
$$;

-- Anonymous callers and signed-in callers alike: the answer holds exactly the sections this viewer
-- may see. A signed-in non-owner counts as one visitor for the day.
create or replace function public.get_public_profile(p_slug text)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_viewer uuid := auth.uid();
  v_owner uuid;
  v_view jsonb;
begin
  if p_slug is null or p_slug !~ '^[a-f0-9]{16}$' then
    return jsonb_build_object('code', 'not_found');
  end if;
  select s.user_id into v_owner from public.profile_settings s where s.slug = p_slug;
  if v_owner is null then
    return jsonb_build_object('code', 'not_found');
  end if;
  if v_viewer is not null and v_viewer <> v_owner then
    insert into public.profile_visits (owner_id, visitor_id, day)
    values (v_owner, v_viewer, (now() at time zone 'utc')::date)
    on conflict do nothing;
  end if;
  v_view := public.profile_view(v_owner, v_viewer);
  return jsonb_build_object('code', 'ok', 'name', v_view -> 'name', 'sections', v_view -> 'sections');
end;
$$;

-- A room mate's profile by user id (the room screen knows ids, not slugs).
create or replace function public.get_member_profile(p_user uuid)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_viewer uuid := auth.uid();
  v_view jsonb;
begin
  if v_viewer is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if p_user is null or (p_user <> v_viewer and not public.profiles_share_room(p_user, v_viewer)) then
    return jsonb_build_object('code', 'not_member');
  end if;
  if p_user <> v_viewer then
    insert into public.profile_visits (owner_id, visitor_id, day)
    select p_user, v_viewer, (now() at time zone 'utc')::date
    where exists (select 1 from public.profile_settings s where s.user_id = p_user)
    on conflict do nothing;
  end if;
  v_view := public.profile_view(p_user, v_viewer);
  if v_view is null then
    return jsonb_build_object('code', 'not_found');
  end if;
  return jsonb_build_object('code', 'ok', 'name', v_view -> 'name', 'sections', v_view -> 'sections');
end;
$$;

-- ---------------------------------------------------------------- guestbook

-- Only members of a common room may write, and only while the owner's guestbook is not private.
-- At most 5 entries a minute and 30 a day per author.
create or replace function public.sign_guestbook(p_owner uuid, p_body text)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_body text := btrim(coalesce(p_body, ''));
  v_name text;
  v_id bigint;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if length(v_body) not between 1 and 200 then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  if p_owner is null or p_owner = v_user then
    return jsonb_build_object('code', 'cannot_self');
  end if;
  if not public.profiles_share_room(v_user, p_owner) then
    return jsonb_build_object('code', 'not_friends');
  end if;
  if not exists (select 1 from public.profile_settings s where s.user_id = p_owner and s.vis_guestbook <> 'private') then
    return jsonb_build_object('code', 'closed');
  end if;
  if (select count(*) from public.profile_guestbook b where b.author_id = v_user and b.created_at > now() - interval '1 minute') >= 5
     or (select count(*) from public.profile_guestbook b where b.author_id = v_user and b.created_at > now() - interval '1 day') >= 30 then
    return jsonb_build_object('code', 'rate_limited');
  end if;
  select m.nickname into v_name
  from public.room_members m
  where m.user_id = v_user
    and exists (select 1 from public.room_members o where o.room_id = m.room_id and o.user_id = p_owner)
  order by m.joined_at limit 1;
  insert into public.profile_guestbook (owner_id, author_id, author_name, body)
  values (p_owner, v_user, v_name, v_body)
  returning id into v_id;
  return jsonb_build_object('code', 'ok', 'id', v_id);
end;
$$;

-- The owner or the author may delete an entry.
create or replace function public.delete_guestbook_entry(p_id bigint)
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
  delete from public.profile_guestbook where id = p_id and (owner_id = v_user or author_id = v_user);
  get diagnostics v_changed = row_count;
  return jsonb_build_object('code', case when v_changed = 1 then 'ok' else 'not_found' end);
end;
$$;

-- The owner reports an entry: it disappears from everyone else's view (the owner still sees it
-- marked) until the owner deletes it.
create or replace function public.report_guestbook_entry(p_id bigint)
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
  update public.profile_guestbook set reported = true where id = p_id and owner_id = v_user;
  get diagnostics v_changed = row_count;
  return jsonb_build_object('code', case when v_changed = 1 then 'ok' else 'not_found' end);
end;
$$;

-- ---------------------------------------------------------------- gifts

-- The sender's points were already spent locally under the same idempotency key. A repeat of the
-- same key with the same content is answered as a success without a second gift; the same key with
-- other content is refused. Points never travel: only the item id and a short message do.
create or replace function public.send_gift(p_idem_key uuid, p_recipient uuid, p_item_id text, p_message text)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_message text := btrim(coalesce(p_message, ''));
  v_existing public.gifts%rowtype;
  v_name text;
  v_id uuid;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if p_idem_key is null or p_recipient is null or p_item_id is null or length(v_message) > 60 then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  select * into v_existing from public.gifts g where g.sender_id = v_user and g.idem_key = p_idem_key;
  if found then
    if v_existing.recipient_id = p_recipient and v_existing.item_id = p_item_id then
      return jsonb_build_object('code', 'ok', 'gift_id', v_existing.id, 'replayed', true);
    end if;
    return jsonb_build_object('code', 'key_conflict');
  end if;
  if not exists (select 1 from public.gift_catalog c where c.item_id = p_item_id) then
    return jsonb_build_object('code', 'not_giftable');
  end if;
  if p_recipient = v_user then
    return jsonb_build_object('code', 'cannot_self');
  end if;
  if not public.profiles_share_room(v_user, p_recipient) then
    return jsonb_build_object('code', 'not_friends');
  end if;
  if (select count(*) from public.gifts g where g.sender_id = v_user and g.created_at > now() - interval '1 day') >= 5 then
    return jsonb_build_object('code', 'daily_limit');
  end if;
  select m.nickname into v_name
  from public.room_members m
  where m.user_id = v_user
    and exists (select 1 from public.room_members o where o.room_id = m.room_id and o.user_id = p_recipient)
  order by m.joined_at limit 1;
  insert into public.gifts (idem_key, sender_id, recipient_id, sender_name, item_id, message)
  values (p_idem_key, v_user, p_recipient, v_name, p_item_id, v_message)
  returning id into v_id;
  return jsonb_build_object('code', 'ok', 'gift_id', v_id, 'replayed', false);
end;
$$;

-- Whether a send with this key reached the server. The sender uses it before refunding after an
-- unclear failure; "none" is only ever returned as a successful answer.
create or replace function public.gift_status(p_idem_key uuid)
returns jsonb
language plpgsql
stable
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_status text;
  v_id uuid;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  select g.status, g.id into v_status, v_id from public.gifts g where g.sender_id = v_user and g.idem_key = p_idem_key;
  if v_id is null then
    return jsonb_build_object('code', 'ok', 'state', 'none');
  end if;
  return jsonb_build_object('code', 'ok', 'state', v_status, 'gift_id', v_id);
end;
$$;

-- The gift box: gifts waiting for an answer plus those accepted in the last 30 days (so an accept
-- whose reply was lost can be applied locally again).
create or replace function public.list_gift_inbox()
returns jsonb
language plpgsql
stable
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  return jsonb_build_object('code', 'ok', 'gifts', coalesce((
    select jsonb_agg(jsonb_build_object(
             'gift_id', g.id, 'item_id', g.item_id, 'sender_name', g.sender_name, 'message', g.message,
             'status', g.status, 'at_ms', (extract(epoch from g.created_at) * 1000)::bigint)
           order by g.created_at)
    from public.gifts g
    where g.recipient_id = v_user
      and (g.status = 'pending' or (g.status = 'accepted' and g.resolved_at > now() - interval '30 days'))
  ), '[]'::jsonb));
end;
$$;

create or replace function public.accept_gift(p_gift_id uuid)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_gift public.gifts%rowtype;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  select * into v_gift from public.gifts g where g.id = p_gift_id and g.recipient_id = v_user for update;
  if not found then
    return jsonb_build_object('code', 'not_found');
  end if;
  if v_gift.status = 'declined' then
    return jsonb_build_object('code', 'already_resolved');
  end if;
  if v_gift.status = 'pending' then
    update public.gifts set status = 'accepted', resolved_at = now() where id = v_gift.id;
  end if;
  return jsonb_build_object('code', 'ok', 'item_id', v_gift.item_id, 'sender_name', v_gift.sender_name);
end;
$$;

create or replace function public.decline_gift(p_gift_id uuid)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_gift public.gifts%rowtype;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  select * into v_gift from public.gifts g where g.id = p_gift_id and g.recipient_id = v_user for update;
  if not found then
    return jsonb_build_object('code', 'not_found');
  end if;
  if v_gift.status = 'accepted' then
    return jsonb_build_object('code', 'already_resolved');
  end if;
  if v_gift.status = 'pending' then
    update public.gifts set status = 'declined', resolved_at = now() where id = v_gift.id;
  end if;
  return jsonb_build_object('code', 'ok');
end;
$$;

-- ---------------------------------------------------------------- function privileges

revoke execute on function public.profiles_share_room(uuid, uuid) from public, anon, authenticated;
revoke execute on function public.profile_scope_allows(text, uuid, uuid) from public, anon, authenticated;
revoke execute on function public.profile_view(uuid, uuid) from public, anon, authenticated;

revoke execute on function public.set_profile_settings(text, text, text, text, text, text, text, text, text, text, integer, text[], text[], text[], text) from public, anon;
revoke execute on function public.report_profile_grass(jsonb) from public, anon;
revoke execute on function public.get_my_profile() from public, anon;
revoke execute on function public.get_public_profile(text) from public;
revoke execute on function public.get_member_profile(uuid) from public, anon;
revoke execute on function public.sign_guestbook(uuid, text) from public, anon;
revoke execute on function public.delete_guestbook_entry(bigint) from public, anon;
revoke execute on function public.report_guestbook_entry(bigint) from public, anon;
revoke execute on function public.send_gift(uuid, uuid, text, text) from public, anon;
revoke execute on function public.gift_status(uuid) from public, anon;
revoke execute on function public.list_gift_inbox() from public, anon;
revoke execute on function public.accept_gift(uuid) from public, anon;
revoke execute on function public.decline_gift(uuid) from public, anon;

grant execute on function public.set_profile_settings(text, text, text, text, text, text, text, text, text, text, integer, text[], text[], text[], text) to authenticated;
grant execute on function public.report_profile_grass(jsonb) to authenticated;
grant execute on function public.get_my_profile() to authenticated;
grant execute on function public.get_public_profile(text) to anon, authenticated;
grant execute on function public.get_member_profile(uuid) to authenticated;
grant execute on function public.sign_guestbook(uuid, text) to authenticated;
grant execute on function public.delete_guestbook_entry(bigint) to authenticated;
grant execute on function public.report_guestbook_entry(bigint) to authenticated;
grant execute on function public.send_gift(uuid, uuid, text, text) to authenticated;
grant execute on function public.gift_status(uuid) to authenticated;
grant execute on function public.list_gift_inbox() to authenticated;
grant execute on function public.accept_gift(uuid) to authenticated;
grant execute on function public.decline_gift(uuid) to authenticated;
