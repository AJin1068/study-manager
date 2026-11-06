# Study Manager browser extension (Chrome, MV3)

Sends the app one message when the active tab changes or its address changes, and replaces the
page with `blocked.html` when the app answers `block`. It never sends on a timer. It sends the
tab title (at most 120 characters) only for a tab the user just switched to; the app keeps it in
memory for the optional LLM tab judge (off by default) and stores only the domain. If the app is not connected the extension allows
every page (fail-open) and shows `끊김` on the toolbar icon.

## Build and load (unpacked)

```sh
pnpm build:extension          # type check + bundle into extension/dist
```

Chrome: `chrome://extensions` -> Developer mode -> Load unpacked -> select `extension/dist`.
The manifest `key` pins the extension ID to `manoifclikefdhhomeklmdjbeknmloem`; the app lists the
same ID in `ALLOWED_EXTENSION_IDS` (`src-tauri/src/browser_link/mod.rs`) and in the native
messaging host manifest `allowed_origins`. Only the public key is in the repo.

The app registers the native messaging host (`com.studymanager.app.browser`) itself; this
extension does not install anything outside the browser.

## Tests

`pnpm test` runs the extension unit tests with a fake `chrome` API (`src/fakeChrome.ts`).

## Not verified here

Loading in a real Chrome, `connectNative` against the app, redirect flicker, service worker
lifetime with the port open, incognito behaviour, and bypass by disabling the extension (the app
records the disconnect).
