-- Stage 5 (S5.7): the phone_app notification channel.
--
-- The channel value 'phone_app' already exists in the core check. It is delivered through the
-- user's ntfy topic (the phone app subscribes to the same topic and server the ntfy channel
-- uses), so a phone_app row without a topic can never send anything. The Edge Function skips such
-- a row; this constraint stops it from being stored at all.
--
-- NOT VALID: rows written before this migration are not re-checked (an old phone_app row without
-- a topic is harmless: it is skipped), every insert and update from now on is.
--
-- No policy or grant changes: notify_channels keeps its owner-only policies.

alter table public.notify_channels
  add constraint notify_channels_phone_app_needs_topic
  check (channel <> 'phone_app' or ntfy_topic is not null) not valid;
