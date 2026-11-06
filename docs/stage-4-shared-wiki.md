# Stage 4: shared wiki (S4.7) and Realtime adapter (S4.12)

## S4.7 acceptance map

| AC | Where it is checked |
| --- | --- |
| A page without a share mark is in no payload (canary) | `shared_wiki::scenarios::a_page_is_private_by_default...`, `payload::tests` |
| Sharing needs the confirmation screen; the merge flag is separate and defaults off | `scenarios::sharing_needs_the_confirmation...`, `the_merge_flag_defaults_off...` |
| Only consented text reaches the model (egress capture), private pages have no way in | `scenarios::only_consented_contributions...`, `merge::tests` (`ConsentedPart` has no constructor from local text) |
| The merge keeps the contributor list and does not overwrite originals | `scenarios::only_consented_contributions...` (server credit lists only consenting authors; revision count +1) |
| Injection pages stay data | `scenarios::an_injection_page...`, `merge::tests::injection_...` |
| Withdrawing: server text gone, no new revision, friends' copy dropped | `scenarios::withdrawing_removes...` |
| Concurrent edits are branches, not last-write-wins | `scenarios::a_concurrent_edit...` |
| Size and count limits | `scenarios::size_limits...`, SQL checks in `supabase/tests/shared_wiki.static.test.ts` |
| New concept notice from local page names, announced once | `scenarios::a_friends_page_creates...`, `a_concept_the_user_already_has...`, `concepts::tests` |
| Mutual quiz without the model | `scenarios::a_mutual_quiz...`, `quiz::tests` |
| Daily recap | `scenarios::the_daily_summary...`, `summary::tests` |
| Non-members get nothing | `scenarios::a_non_member_gets_nothing` |
| Server SQL structure (RLS on, no policy, definer pinned/revoked, consent + credit in `save_merged_page`) | `supabase/tests/shared_wiki.static.test.ts` |

Privacy: only the page name and text of pages the user marked, plus the merge flag, leave the PC. Friends see
nothing else from this feature; deviation statistics are not part of it.

## S4.12 Realtime

`rooms::realtime` holds the Phoenix-channel codec, the session state machine (join, heartbeat every 30 s,
loss after 65 s of silence) and the in-memory transport pair used by the tests. A change notice only makes
`PollingFeed::poll_now` fire, so the data still comes from the ordinary RPCs and row policies. No websocket
crate is in the project, so there is no socket-backed transport yet; polling stays the default (O-12).

## Not verified

- V4-1: the SQL and `supabase/tests/rls_s4_shared_wiki.sql` were not run (no Supabase CLI/Docker). The
  in-memory server mirrors the SQL; it does not prove it.
- V4-4: real merge quality with a live model, and real revision concurrency on a server.
- V4-5: a real websocket connection to Supabase Realtime (no transport exists yet).
- The shared-wiki UI (`ShareToRoom`, `SharedWikiPanel`) was type-checked and linted, not exercised in the app.
