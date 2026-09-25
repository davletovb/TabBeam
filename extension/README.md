# Pervue Chrome Extension

This directory contains the Manifest V3 browser extension.

## Development

1. Open `chrome://extensions`.
2. Enable **Developer mode**.
3. Choose **Load unpacked**.
4. Select this `extension/` directory.

The current foundation provides:

- toolbar popup;
- full-page extension route;
- MV3 module service worker;
- HTTP/HTTPS content script for explicit selection and readable-page capture;
- keyboard shortcut to the focused popup composer;
- selection and current-page context-menu actions;
- service-worker-owned Native Messaging connection manager;
- popup and full-page ask/stream UI backed by the native host;
- versioned local conversation history with a recent-conversation picker;
- dependency-free smoke/lifecycle validation.

## Native Messaging connection lifecycle

`src/background/native-connection.js` owns the browser-side native port lifecycle.

- The native port opens lazily on the first request.
- One module-scope manager instance lives in the service worker, so popup closure does not own or tear down the native connection.
- In-flight requests are multiplexed by protocol `request_id`. `send()` rejects IDs outside the protocol v1 grammar before opening the port, because the host could not echo them back to the right request.
- `send()` serializes `request_id` as the first envelope member, so the host can still echo it when a later member is malformed or nested too deeply (protocol v1 §8.4).
- `send()` measures each request as Chrome sends it (UTF-8 JSON) and refuses one over the host's 1 MiB frame limit with `RequestTooLargeError`, or one JSON can't represent, before the port opens or is used. The host would treat an oversized frame as a broken stream and exit, failing every request in flight; now a bad request fails on its own. The limit lives in `src/shared/limits.js` and is tested against `docs/protocol/native-messaging-v1.json` (SEC-01).
- Terminal protocol events release their request route before the owner's handler runs, so the owner can reuse the request ID or disconnect.
- Native-port disconnect clears in-flight routes and notifies every request owner.
- Requester and listener callbacks are isolated: a callback that throws is reported (by default with `console.error`) and cannot stop other owners from being notified or the native port from closing.
- A subsequent request reconnects automatically.
- Stale callbacks from an old port are ignored after a replacement connection is established.

The canonical Native Messaging host name is currently `com.pervue.host`. Packaging/registration work later in the tracker must register the companion under that same name.

## Popup ask flow

The popup and full-page view ask the native host one question at a time and stream the answer into a shared conversation.

- The input is focused and usable as soon as the popup opens. Enter asks; Shift+Enter adds a line; an Enter that ends an IME composition does neither.
- Each question opens its own runtime port to the service worker (contract: `src/shared/ask-port.js`). The service worker sends one `conversation.send`, persists the completed turn, and forwards that request's protocol events in order, ending with exactly one terminal event.
- While a question is in flight, every other submit is ignored, whether it comes from Enter, the Ask button or `requestSubmit()`. The input stays editable.
- Deltas are appended as text nodes as they arrive, so provider output is never parsed as HTML.
- Completion and failure show in the status line without reloading. Failures show the error's `message`.
- A question over the native host's 1 MiB limit is refused in the popup before it's sent ("Your question is too long…"). The service worker checks the whole request too, and reports `INVALID_REQUEST` / `REQUEST_TOO_LARGE`.
- When the native port closes before the answer finishes, the service worker reports the failure itself, in the DOC-02 vocabulary, based on Chrome's `runtime.lastError`. A missing host, or one registered only for other extensions, is `HOST_NOT_INSTALLED` and not retryable. A host that can't start or that disconnects is `HOST_UNAVAILABLE`.
- Closing the popup leaves the native request running; its answer is saved when it finishes. Reopen the popup to see the recent thread. Cancellation is EXT-12.
- Only the extension's own pages can open the ask port. The service worker disconnects ports from content scripts.
- New conversations use Codex. The extension stores a stable `conv_<UUID>` ID, message history, title, source records, and optional page-context metadata under a versioned `chrome.storage.local` key, keeping the 30 most recent threads. Native provider session IDs remain private to the service worker and adapter. The recent picker loads a saved thread; **New conversation** clears the active thread; **Continue in full view** opens the same ID in the larger view. Follow-ups send bounded prior dialogue so the adapter can continue even after a native session is lost. One question per conversation runs at a time across both views.

## Browser context (CTX-01 through CTX-04)

The popup starts with **No context**. To attach context, click **Use selection** after selecting text on the page, or **Use this page** to extract readable text. The popup shows the page title, sanitized URL, a text preview, and whether capture was truncated. Press **Ask** to send the question and the chosen context together; **No context** clears it. A fresh popup starts without context. An attached capture applies to that question only. Saved conversations retain only page title, sanitized URL, mode, and truncation metadata, never the raw captured text. No capture happens on popup open or a normal Ask.

Right-click selected text and choose **Ask Pervue about selection**, or right-click a page and choose **Ask Pervue about this page**. The menu opens the popup with that context previewed; it does not ask automatically. Chrome's menu selection includes text selected within frames. The handoff belongs to the clicked tab, expires after 30 seconds, and can be claimed once. If Chrome cannot open the action popup, the same composer opens in a tab with only an opaque handoff token in its URL. **Alt+Shift+P** (or **Command+Shift+P** on macOS) invokes the popup directly and focuses the input; Chrome users can change the shortcut under `chrome://extensions/shortcuts`.

The content script responds only to explicit capture messages. Selection supports regular text and focused text-field selections, up to 16 KiB of UTF-8. Page extraction prefers readable text from `main` or `article`, falls back to `body` when those are empty, skips scripts, navigation, hidden content, and form/editable surfaces, and stops at 64 KiB of UTF-8, 5,000 text nodes, or 512 Ki UTF-16 units examined in total. Direct popup capture is limited to the top frame; the menu selection comes from Chrome's click event. The service worker checks the response size again, gets the tab's title and URL, keeps at most 256 title code points and 2,048 URL bytes, removes URL credentials, query, and fragment, and rejects unsupported pages. Failed capture leaves Ask usable without context. Permission and unavailability outcomes use `CONTEXT_UNAVAILABLE`, and site access is used only for explicit popup/menu actions; the choice is not persisted. Page/provider output is rendered as text, and callback errors omit raw content from logs.

### Trying it against the local host

Until packaging registers the host (Milestone G), register a development build by hand:

1. Build the host: `cargo build -p pervue-host` in `native/`.
2. Load this directory unpacked and copy the extension ID from `chrome://extensions`.
3. Have the host print its manifest for that ID, and save it as `com.pervue.host.json` in Chrome's per-user `NativeMessagingHosts` directory, creating the directory if needed. For example, on Linux:

   ```bash
   native/target/debug/pervue-host --print-manifest <extension-id> \
     > ~/.config/google-chrome/NativeMessagingHosts/com.pervue.host.json
   ```

   The macOS directory is `~/Library/Application Support/Google/Chrome/NativeMessagingHosts/`. On Windows, save the file anywhere, then create the registry key `HKCU\Software\Google\Chrome\NativeMessagingHosts\com.pervue.host` and set its default value to the file's full path. The manifest allows only the IDs you pass, and its `path` is the host binary that printed it.
4. Open the popup and ask anything. The fake provider answers "Fake provider response."

## Validation

Run the extension validation pipeline with:

```bash
cd extension
npm run lint
npm run typecheck
npm run build
npm test
```

The build emits an unpacked extension under `extension/dist/`. Lint and typecheck dependencies are pinned and fetched on demand by npm, so the repository does not need a generated dependency tree for this vanilla-JS foundation.
