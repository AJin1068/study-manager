-- Stage 2-6: what the missed-plan Edge Function needs to word and address an alert.
--
-- utc_offset_min  lets the server print the plan's start as the user's own clock time (it never
--                 stores a place name, only the current offset; the app refreshes it).
-- ntfy_server     the user's ntfy server (default https://ntfy.sh). The Edge Function re-checks it
--                 before posting: https only, a public host name, no credentials or port.
--
-- No policy or grant changes: notify_channels keeps its owner-only policies from the core migration.

alter table public.notify_channels
  add column utc_offset_min integer not null default 540 check (utc_offset_min between -840 and 840),
  add column ntfy_server    text check (ntfy_server is null or length(ntfy_server) <= 200);
