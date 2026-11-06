-- Stage 5 follow-up: anonymous (phone) accounts may use only the phone-side RPCs.
--
-- The phone signs in with a Supabase anonymous account: role `authenticated` plus the JWT claim
-- is_anonymous = true. Until now only create_pair_code refused such a caller, so a phone account
-- could create rooms, publish a profile, post to guestbooks or send gifts like a real user. This
-- migration puts the same refusal at the top of every other owner-side RPC. It does not change
-- grants (the role is the same) and it does not touch the RPCs a phone needs:
--   claim_pair, phone_lease, phone_beat, report_phone_pickups   (the phone side of the link)
--   get_public_profile                                           (open to anon and authenticated)
--   create_pair_code                                             (already refuses, in its own body)
--
-- Each function is re-created from its current definition with the guard inserted as the first
-- statement of its body, so no body is copied here and the guard cannot drift from the function.
-- The refusal is a returned code like every other refusal ('anonymous_not_allowed'); the two
-- sync RPCs return text, the rest return jsonb {"code": ...}. The list is checked by
-- supabase/tests/anonymous_guard.static.test.ts against the RPC contracts and by
-- supabase/tests/rls_s5_anonymous_guard.sql.

do $anonymous_guard$
declare
  v_signature text;
  v_definition text;
  v_guarded text;
  v_condition constant text := E'  if coalesce((auth.jwt() ->> ''is_anonymous'')::boolean, false) then\n';
  v_jsonb_guard constant text :=
    E'\nbegin\n  -- anonymous_guard\n' || v_condition || E'    return jsonb_build_object(''code'', ''anonymous_not_allowed'');\n  end if;\n';
  v_text_guard constant text :=
    E'\nbegin\n  -- anonymous_guard\n' || v_condition || E'    return ''anonymous_not_allowed'';\n  end if;\n';
begin
  -- jsonb {"code": ...} RPCs
  foreach v_signature in array array[
    -- rooms (S4)
    'public.create_room(text, text)',
    'public.create_invite(uuid, integer, integer)',
    'public.revoke_invite(uuid, uuid)',
    'public.join_room(text, text)',
    'public.leave_room(uuid)',
    'public.kick_member(uuid, uuid)',
    'public.list_my_rooms()',
    'public.room_snapshot(uuid)',
    'public.report_status(text, integer, integer, integer)',
    'public.report_daily(date, integer, integer)',
    'public.set_friend_prefs(boolean, boolean, boolean, boolean, boolean, boolean)',
    'public.set_inventory_public(text, text, text, text, text)',
    'public.report_distraction()',
    'public.poll_distraction_events(bigint)',
    'public.share_schedule(uuid, jsonb)',
    -- shared wiki (S4)
    'public.publish_shared_page(uuid, text, text, uuid, boolean)',
    'public.withdraw_shared_page(uuid, text)',
    'public.list_shared_pages(uuid)',
    'public.save_merged_page(uuid, text, text, uuid[])',
    -- profile, guestbook, gifts (S4)
    'public.set_profile_settings(text, text, text, text, text, text, text, text, text, text, integer, text[], text[], text[], text)',
    'public.report_profile_grass(jsonb)',
    'public.get_my_profile()',
    'public.get_member_profile(uuid)',
    'public.sign_guestbook(uuid, text)',
    'public.delete_guestbook_entry(bigint)',
    'public.report_guestbook_entry(bigint)',
    'public.send_gift(uuid, uuid, text, text)',
    'public.gift_status(uuid)',
    'public.list_gift_inbox()',
    'public.accept_gift(uuid)',
    'public.decline_gift(uuid)',
    -- the owner side of the phone link (S5)
    'public.revoke_pair()',
    'public.phone_status()',
    'public.publish_phone_lock(bigint, text, text, text, bigint)'
  ] loop
    v_definition := pg_get_functiondef(v_signature::regprocedure);
    v_guarded := regexp_replace(v_definition, E'\nbegin\n', v_jsonb_guard);
    if v_guarded = v_definition then
      raise exception 'anonymous guard not inserted into %', v_signature;
    end if;
    execute v_guarded;
  end loop;

  -- text-returning sync RPCs (S2)
  foreach v_signature in array array[
    'public.replace_plan_snapshot(uuid, text, bigint, integer, jsonb)',
    'public.heartbeat(uuid, text, boolean)'
  ] loop
    v_definition := pg_get_functiondef(v_signature::regprocedure);
    v_guarded := regexp_replace(v_definition, E'\nbegin\n', v_text_guard);
    if v_guarded = v_definition then
      raise exception 'anonymous guard not inserted into %', v_signature;
    end if;
    execute v_guarded;
  end loop;
end
$anonymous_guard$;
