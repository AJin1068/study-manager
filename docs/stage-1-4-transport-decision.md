# 4-S0: extension-to-app transport decision

**Decision (D-20 confirmed): native messaging + local IPC.** Chrome starts a small host process
(the app binary itself, recognised by the `chrome-extension://<id>/` argument Chrome passes, handled
in `main.rs` before Tauri starts). The host relays frames between Chrome stdio and a Unix domain
socket in the app data directory (Windows named pipe: not implemented, `Unsupported`).

## Why not a localhost server

| Concern | localhost WebSocket/HTTP | native messaging + local IPC |
| --- | --- | --- |
| Who can connect | any local process or web page | only the host Chrome starts for the pinned `allowed_origins`; the socket is `0600` |
| Secret for the extension | must be pasted by the user or delivered by NM anyway | none: the host adds the install token and the caller origin itself |
| MV3 service worker (idle stop ~30 s) | socket dies with the worker; reconnect needs a wake source | an open `connectNative` port keeps the worker alive (to verify in Chrome) |
| App not running | connection refused, indistinguishable from a network error | host answers `error{app_not_running}`; extension fails open |
| Extension disabled/removed | not visible to the app | the host exits, the app sees EOF and records `extension_disconnected` |

No finding contradicts the default, so it stands.

## Falsification attempts (agent-runnable)

- Host relay round trip (Chrome stdin -> host -> socket -> echo -> host -> stdout), 300 samples over real
  Unix sockets on the dev Mac: median about 14 us, p99 about 25 us
  (`host::tests::relay_round_trip_latency_is_small`, asserts median < 5 ms). The transport adds no
  user-visible delay; the browser-side costs are not measured here.
- App not running: `connect` fails, the host emits one `app_not_running` frame and exits 1
  (`host::tests::app_not_running_is_reported_to_the_extension`, `platform::browser_ipc::tests::socket_round_trip_and_missing_app`).
- App going away ends the host, so Chrome closes the port (`host::tests::relays_both_directions_with_auth_first`).
- Frame parser: truncated header/body, oversized length (rejected before the body is read), corrupted
  and unknown messages (`protocol::tests`).
- Wrong token, wrong or unlisted extension ID, origin/hello mismatch, flood, panicking judge
  (`browser_link::tests`).

## Not verified (needs a person with Chrome)

- Loading an unpacked extension, `connectNative`, and the real manifest in
  `~/Library/Application Support/Google/Chrome/NativeMessagingHosts/` (nothing was installed to the home directory).
- MV3 service worker lifetime with the port open; incognito behaviour.
- A GUI-subsystem Windows binary used as a stdio host, the Windows named-pipe transport, and
  registry registration.
- Edge and Brave manifest locations (only Chrome's macOS directory is coded).
- `ALLOWED_EXTENSION_IDS` now lists the ID pinned by the manifest `key` (`manoifclikefdhhomeklmdjbeknmloem`, derived from the key's SHA-256 outside the test suite); that the real Chrome computes the same ID is not verified.

## Transport replacement

`browser_link::protocol` (frames, messages) and `browser_link::service` (auth, judging, recording)
only see `platform::LinkStream`, so the socket can be swapped without touching them.
