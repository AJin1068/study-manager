-- Stage 4 (S4.7): shared wiki pages of a study room.
--
-- Rules
--   * A page exists in a room only because a member explicitly published it. Nothing here is read
--     from the member's private wiki: the app sends text only for pages the user marked as shared.
--   * Revisions are append-only. Publishing never overwrites another member's text: every member's
--     latest revision is kept side by side. A publish whose base revision is not the page's newest
--     one is stored as a branch (branched = true) and is a merge input, not a lost update.
--   * Withdrawing is a tombstone: the author's revisions get withdrawn_at and an empty body, so the
--     text can no longer be read from the server. A merged revision lists its contributors; when a
--     contributor withdraws, that merge is tombstoned too. A copy someone already saw cannot be
--     taken back (the app says so before sharing).
--   * allow_llm_merge is the author's separate consent that others may send this text to an LLM
--     when merging. save_merged_page accepts only sources whose author consented, and derives the
--     contributor list itself from those sources, so credit cannot be forged or dropped.
--   * Clients can NOT read or write these tables. RLS is on with no policy and every privilege is
--     revoked; reads and writes go through the RPCs below (security definer, search_path pinned,
--     EXECUTE revoked from public and anon, caller identified with auth.uid(), membership checked).
--   * Someone who left the room (or was kicked) no longer appears: list_shared_pages hides their
--     revisions, and merges that credit them.
--   * Limits: title 1-80 characters, body 1-20000 characters, 20 pages per member per room, 100
--     pages per room, 50 live revisions per member per page, 500 per page.
-- Every RPC returns jsonb {"code": "...", ...} and never RAISEs.

create table public.shared_pages (
  id         uuid primary key default gen_random_uuid(),
  room_id    uuid not null references public.rooms (id) on delete cascade,
  title      text not null check (length(title) between 1 and 80),
  created_at timestamptz not null default now(),
  unique (room_id, title)
);

create table public.shared_page_revisions (
  id              uuid primary key default gen_random_uuid(),
  page_id         uuid not null references public.shared_pages (id) on delete cascade,
  user_id         uuid not null default auth.uid() references auth.users (id) on delete cascade,
  kind            text not null check (kind in ('edit', 'merge')),
  body            text not null check (length(body) <= 20000),
  base_revision   uuid references public.shared_page_revisions (id) on delete set null,
  branched        boolean not null default false,
  allow_llm_merge boolean not null default false,
  contributor_ids uuid[] not null default '{}',
  created_at      timestamptz not null default now(),
  withdrawn_at    timestamptz,
  check ((withdrawn_at is null and length(body) >= 1) or (withdrawn_at is not null and body = ''))
);
create index shared_page_revisions_page on public.shared_page_revisions (page_id, created_at);

alter table public.shared_pages          enable row level security;
alter table public.shared_page_revisions enable row level security;

revoke all on public.shared_pages, public.shared_page_revisions from public, anon, authenticated;

-- ---------------------------------------------------------------- publish

create or replace function public.publish_shared_page(
  p_room_id uuid, p_title text, p_body text, p_base_revision uuid, p_allow_llm_merge boolean)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_title text := btrim(coalesce(p_title, ''));
  v_page uuid;
  v_head uuid;
  v_mine integer;
  v_has_mine boolean;
  v_rev uuid;
  v_branched boolean := false;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if not public.is_room_member(p_room_id, v_user) then
    return jsonb_build_object('code', 'not_member');
  end if;
  if length(v_title) not between 1 and 80 or v_title ~ '[[:cntrl:]]' or p_allow_llm_merge is null then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  if p_body is null or length(btrim(p_body)) = 0 then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  if length(p_body) > 20000 then
    return jsonb_build_object('code', 'too_large');
  end if;

  select id into v_page from public.shared_pages where room_id = p_room_id and title = v_title;
  if v_page is null then
    if (select count(*) from public.shared_pages where room_id = p_room_id) >= 100 then
      return jsonb_build_object('code', 'too_many');
    end if;
  else
    if p_base_revision is not null
       and not exists (select 1 from public.shared_page_revisions r where r.id = p_base_revision and r.page_id = v_page) then
      return jsonb_build_object('code', 'invalid_input');
    end if;
    if (select count(*) from public.shared_page_revisions r where r.page_id = v_page) >= 500 then
      return jsonb_build_object('code', 'too_many');
    end if;
  end if;

  select count(distinct p.id) into v_mine
  from public.shared_pages p
  join public.shared_page_revisions r on r.page_id = p.id
  where p.room_id = p_room_id and r.user_id = v_user and r.kind = 'edit' and r.withdrawn_at is null
    and p.id is distinct from v_page;
  if v_mine >= 20 then
    return jsonb_build_object('code', 'too_many');
  end if;

  if v_page is null then
    insert into public.shared_pages (room_id, title) values (p_room_id, v_title) returning id into v_page;
  else
    if (select count(*) from public.shared_page_revisions r
         where r.page_id = v_page and r.user_id = v_user and r.kind = 'edit' and r.withdrawn_at is null) >= 50 then
      return jsonb_build_object('code', 'too_many');
    end if;
    select r.id into v_head from public.shared_page_revisions r
      where r.page_id = v_page and r.withdrawn_at is null
      order by r.created_at desc, r.id desc limit 1;
    select exists (select 1 from public.shared_page_revisions r where r.page_id = v_page and r.user_id = v_user and r.kind = 'edit')
      into v_has_mine;
    v_branched := v_head is not null and p_base_revision is distinct from v_head and (p_base_revision is not null or v_has_mine);
  end if;

  insert into public.shared_page_revisions (page_id, user_id, kind, body, base_revision, branched, allow_llm_merge, contributor_ids)
  values (v_page, v_user, 'edit', p_body, p_base_revision, v_branched, p_allow_llm_merge, array[v_user])
  returning id into v_rev;
  return jsonb_build_object('code', 'ok', 'page_id', v_page, 'revision_id', v_rev, 'branched', v_branched);
end;
$$;

-- ---------------------------------------------------------------- withdraw

create or replace function public.withdraw_shared_page(p_room_id uuid, p_title text)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_page uuid;
  v_count integer;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if not public.is_room_member(p_room_id, v_user) then
    return jsonb_build_object('code', 'not_member');
  end if;
  select id into v_page from public.shared_pages where room_id = p_room_id and title = btrim(coalesce(p_title, ''));
  if v_page is null then
    return jsonb_build_object('code', 'not_found');
  end if;
  update public.shared_page_revisions
     set body = '', withdrawn_at = now()
   where page_id = v_page and withdrawn_at is null and v_user = any (contributor_ids);
  get diagnostics v_count = row_count;
  if v_count = 0 then
    return jsonb_build_object('code', 'not_shared');
  end if;
  return jsonb_build_object('code', 'ok', 'withdrawn', v_count);
end;
$$;

-- ---------------------------------------------------------------- list

create or replace function public.list_shared_pages(p_room_id uuid)
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_pages jsonb;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if not public.is_room_member(p_room_id, v_user) then
    return jsonb_build_object('code', 'not_member');
  end if;
  select coalesce(jsonb_agg(jsonb_build_object('page_id', q.id, 'title', q.title, 'revisions', q.revs) order by q.title), '[]'::jsonb)
    into v_pages
  from (
    select p.id, p.title,
      (select jsonb_agg(jsonb_build_object(
                'revision_id', x.id,
                'kind', x.kind,
                'author', coalesce((select m.nickname from public.room_members m where m.room_id = p_room_id and m.user_id = x.user_id), ''),
                'is_mine', x.user_id = v_user,
                'body', x.body,
                'allow_llm_merge', x.allow_llm_merge,
                'branched', x.branched,
                'base_revision_id', x.base_revision,
                'contributors', (select coalesce(jsonb_agg(m.nickname order by m.joined_at, m.user_id), '[]'::jsonb)
                                   from public.room_members m
                                  where m.room_id = p_room_id and m.user_id = any (x.contributor_ids)),
                'created_at_ms', (extract(epoch from x.created_at) * 1000)::bigint
              ) order by x.created_at, x.id)
         from (
           (select distinct on (r.user_id) r.*
              from public.shared_page_revisions r
             where r.page_id = p.id and r.kind = 'edit' and r.withdrawn_at is null
               and public.is_room_member(p_room_id, r.user_id)
             order by r.user_id, r.created_at desc, r.id desc)
           union all
           (select r.*
              from public.shared_page_revisions r
             where r.page_id = p.id and r.kind = 'merge' and r.withdrawn_at is null
               and not exists (select 1 from unnest(r.contributor_ids) c where not public.is_room_member(p_room_id, c))
             order by r.created_at desc, r.id desc limit 1)
         ) x) as revs
    from public.shared_pages p
    where p.room_id = p_room_id
  ) q
  where q.revs is not null;
  return jsonb_build_object('code', 'ok', 'pages', v_pages);
end;
$$;

-- ---------------------------------------------------------------- save a merged revision

create or replace function public.save_merged_page(p_room_id uuid, p_title text, p_body text, p_source_revisions uuid[])
returns jsonb
language plpgsql
security definer
set search_path = public, pg_temp
as $$
declare
  v_user uuid := auth.uid();
  v_page uuid;
  v_sources uuid[];
  v_contributors uuid[];
  v_good integer;
  v_rev uuid;
begin
  if v_user is null then
    return jsonb_build_object('code', 'not_signed_in');
  end if;
  if not public.is_room_member(p_room_id, v_user) then
    return jsonb_build_object('code', 'not_member');
  end if;
  select id into v_page from public.shared_pages where room_id = p_room_id and title = btrim(coalesce(p_title, ''));
  if v_page is null then
    return jsonb_build_object('code', 'not_found');
  end if;
  if p_body is null or length(btrim(p_body)) = 0 or p_source_revisions is null
     or cardinality(p_source_revisions) not between 1 and 10 then
    return jsonb_build_object('code', 'invalid_input');
  end if;
  if length(p_body) > 20000 then
    return jsonb_build_object('code', 'too_large');
  end if;
  select array_agg(distinct s) into v_sources from unnest(p_source_revisions) s;
  select count(*), array_agg(distinct r.user_id) into v_good, v_contributors
    from public.shared_page_revisions r
   where r.id = any (v_sources) and r.page_id = v_page and r.kind = 'edit' and r.withdrawn_at is null
     and r.allow_llm_merge and public.is_room_member(p_room_id, r.user_id);
  if v_good is distinct from cardinality(v_sources) then
    return jsonb_build_object('code', 'not_consented');
  end if;
  if (select count(*) from public.shared_page_revisions r where r.page_id = v_page) >= 500 then
    return jsonb_build_object('code', 'too_many');
  end if;
  insert into public.shared_page_revisions (page_id, user_id, kind, body, allow_llm_merge, contributor_ids)
  values (v_page, v_user, 'merge', p_body, false, v_contributors)
  returning id into v_rev;
  return jsonb_build_object('code', 'ok', 'revision_id', v_rev);
end;
$$;

-- ---------------------------------------------------------------- privileges

revoke execute on function public.publish_shared_page(uuid, text, text, uuid, boolean) from public, anon;
revoke execute on function public.withdraw_shared_page(uuid, text) from public, anon;
revoke execute on function public.list_shared_pages(uuid) from public, anon;
revoke execute on function public.save_merged_page(uuid, text, text, uuid[]) from public, anon;

grant execute on function public.publish_shared_page(uuid, text, text, uuid, boolean) to authenticated;
grant execute on function public.withdraw_shared_page(uuid, text) to authenticated;
grant execute on function public.list_shared_pages(uuid) to authenticated;
grant execute on function public.save_merged_page(uuid, text, text, uuid[]) to authenticated;
