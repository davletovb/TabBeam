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
- popup provider state and failure states (EXT-04);
- web search with numbered, checked sources in the popup and full view (EXT-16, EXT-17, SEC-05);
- dependency-free smoke/lifecycle validation.

## Interface

The popup, full view, and setup page share one design system in `src/shared/`:

- `ui.css` holds the color, type, and spacing tokens. Every color is written with `light-dark()`, so the theme follows `color-scheme`: the system preference by default, or the choice stored under `pervue.theme` and applied as `<html data-theme>` by `theme.js` (and before first paint by `theme-bootstrap.js`).
- `icons.svg` is the icon sprite; pages reference it with `<use href="../shared/icons.svg#name">`.
- `theme-toggle.js` draws the System / Light / Dark switch in front of the theme `<select>` that `theme.js` binds, so the select stays the single source of truth.
- `thread-view.js` grows the composer with its text and keeps a streaming answer in view while the reader is at the bottom of the thread.
- `search-toggle.js` is the composer's **Web** switch, `sources.js` checks and bounds an answer's sources, and `source-list.js` draws them.

The popup is a 400 × 580 command surface: a header with the provider status and actions (History, new conversation, continue in full view, and a settings menu holding the theme switch, a link to provider & setup, and diagnostics), the thread, and a docked composer with the **Web** search switch and context chips. The chosen context shows its name; the other choices are icons with tooltips. History replaces the thread while it's open.

The AI provider (and, where the companion supports it, the model) is chosen on the **Provider & setup** page (`src/setup/provider-settings.js`), which also shows each provider's status. The popup and full view follow the saved choice (`pervue.provider`), including a change made while they're open; a conversation keeps the provider it started with, and an unavailable choice falls back to one that's ready. Model choice follows the provider's `model_selection` capability (`src/shared/models.js`). Where a provider can switch, the page lists the models its status suggests (Claude's `sonnet`, `opus`, and `haiku` aliases) and "Other model…" for any valid model ID, saved per provider under `pervue.models`; the default removes the choice. A question sends its provider's saved model as `conversation.send` `model` unless that provider's status says it can't switch; a question asked before the status check finishes still sends it, so a saved choice is never dropped silently; the worker and the host both validate the ID (1–128 letters, digits, `.`, `_`, `-`, `:`, `/`, `@`, never a leading `-`), and the host refuses a model for a provider that can't switch (`MODEL_SELECTION_UNSUPPORTED`).

A new popup starts a new conversation (`src/popup/session.js`), since opening it usually means a new question, often about another page. The exception is picking up where you were: Chrome closes the popup whenever you click the page, so reopening it over the same tab and page within 30 minutes resumes that conversation. The tab → conversation map is kept in `chrome.storage.session` (memory only), keyed by the page's origin and path. Recent conversations are listed on the start screen, one click away. Menu handoffs always start fresh. The full view adds a sidebar of recent conversations, which becomes a drawer in narrow windows; press <kbd>/</kbd> to jump to its composer. Finished answers offer **Copy**.

Pervue's answers render as Markdown (`src/shared/markdown.js`): paragraphs, headings, bullet, numbered, and task lists, fenced code with a copy button, tables, block quotes, rules, and inline code, emphasis, strikethrough, and links. The renderer builds DOM nodes and never parses HTML: raw HTML in an answer stays visible as text, a link keeps its target only if it is an absolute `http`, `https`, or `mailto` URL (and opens in a new tab), and an image becomes a link to its source instead of loading it. What you type stays plain text.

**Web search** (EXT-16, EXT-17). The **Web** switch, first in the composer, asks the chosen provider to search the web and cite what it found (`conversation.send` `search`; Codex and Claude search with their own sign-in, and Pervue holds no search API key). It's on by default and stays as you leave it; it's unavailable for a provider whose status says it can't search, and your choice comes back with one that can. It doesn't combine with page context: turning it on removes the context, and sharing a selection or the page (by a chip, a suggestion, or the context menu) turns it off. A search turn asks the provider to search, cite each page as a link, and answer without narrating; narration a provider writes before searching anyway is left out of the answer. While a search answer is on its way the status reads *Searching the web…* and its sources appear as they're found; the saved answer keeps them. The popup shows them as compact numbered chips (site names, with the title in a tooltip), up to four plus a **+N** chip that continues in the full view; the full view shows every source as a card with its title, site, publisher, age, and excerpt. Both number an answer's sources in the order the provider found them, so a source has the same number and identity in either view. A question asked with search is marked *Searched the web*, and **Retry** repeats its search mode: a search question is retried without page context, as it was asked, while any other question re-reads the current context choice, so context removed since is never sent again. A search that fails, or finds nothing it can cite, says why and keeps no sources, and the conversation carries on.

Sources are untrusted (SEC-05, `docs/security/trust-boundaries.md` §6): the worker accepts one only with an `http`/`https` URL that has a host and no credentials, bounds its text, drops repeats, keeps at most 20 per answer, and passes pages only that checked copy; pages check again and show every field as text. A source opens in a new tab without a referrer, and nothing is fetched to show one.

Answers type out as they arrive (`src/shared/stream-reveal.js`). Codex's `exec --json` mode reports each message whole when it completes rather than token by token, so without this an answer would appear all at once; the reveal paces whatever has arrived to show within about a second, finishing words rather than splitting them, and the saved copy replaces it only once it has finished. With reduced motion, or in a hidden tab, text appears at once.

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
- Deltas accumulate into the answer, which the Markdown renderer draws with DOM nodes, so provider output is never parsed as HTML.
- Completion and failure show in the status line without reloading. A failure shows the error's `message`, which the host and the service worker write to say what to do next, and the line names the failure's kind in `data-kind` (see [Provider state and failures](#provider-state-and-failures)).
- A question over the native host's 1 MiB limit is refused in the popup before it's sent ("Your question is too long…"). The service worker checks the whole request too, and reports `INVALID_REQUEST` / `REQUEST_TOO_LARGE`.
- When the native port closes before the answer finishes, the service worker reports the failure itself, in the DOC-02 vocabulary, based on Chrome's `runtime.lastError`. A missing host, or one registered only for other extensions, is `HOST_NOT_INSTALLED` and not retryable. A host that can't start or that disconnects is `HOST_UNAVAILABLE`.
- Closing the popup leaves the native request running; its answer is saved when it finishes. Reopen the popup to see the recent thread. Cancellation is EXT-12.
- Only the extension's own pages can open the ask port. The service worker disconnects ports from content scripts.
- New conversations use Codex. The extension stores a stable `conv_<UUID>` ID, message history, title, source records, and optional page-context metadata in versioned `chrome.storage.local` records, keeping up to 30 recent threads within a 6 MiB budget. Individual stored messages and long conversations are bounded; older turns may be pruned and large messages show a truncation note. On a quota error it evicts older conversations and retries. Native provider session IDs remain private to the service worker and adapter. **History** (the clock icon in the popup, the sidebar in the full view) lists saved threads by day, with search and a two-click delete; the worker refuses to delete a thread whose question is still running. **New conversation** clears the active thread; **Continue in full view** opens the same ID in the larger view. Follow-ups send bounded prior dialogue so the adapter can continue even after a native session is lost. One question per conversation runs at a time across both views.
- Both views ask Codex, the first real provider (`DEFAULT_PROVIDER_ID` in `src/shared/providers.js`). Choosing among providers is EXT-15.

## Provider state and failures

When the popup opens, a line under the Pervue name shows the state of the companion app and of Codex (EXT-04). The popup sends `{type: "pervue.provider-status"}` to the service worker (contract: `src/shared/provider-status.js`), which asks the host for `provider.status` (`src/background/status-bridge.js`) and passes on only its normalized fields. The line reads "Checking Codex…", then one of:

| State | Line |
|---|---|
| Ready | Codex is ready. |
| Codex not installed | Codex isn't installed. Install it, then try again. |
| Codex not signed in | Codex isn't signed in. Sign in to Codex, then try again. |
| Codex can't start | Codex is installed but can't start. Reinstall it, then try again. |
| Companion app missing, or unreachable | The failure's own message, such as "Pervue's companion app isn't installed. Install it, then try again." |
| Companion app not answering within 15 seconds | Pervue's companion app didn't answer in time. Try again. |
| No answer from the service worker within 20 seconds, or an unknown state | Pervue couldn't check Codex. |

Before any request is sent, the native connection waits for `host.ready` and its supported protocol versions. If protocol 1 is absent, queued requests fail with `HOST_UNAVAILABLE` / `HOST_PROTOCOL_MISMATCH` and the setup action offers a matching companion package; a host that never becomes ready fails after five seconds. A status or error that doesn't follow DOC-02 is also treated as an incompatible host. The service worker waits 15 seconds for a provider status answer (`PROVIDER_STATUS_TIMEOUT_MS`); the host gives up on a stuck sign-in check after 10 seconds. Asking stays possible after a failed check because the state may change after installing or signing in.

A failed question's status line shows the error's message and its kind, decided by the DOC-02 `code` alone (`src/shared/outcomes.js`), never by the message:

| `code` | `data-kind` | How it looks |
|---|---|---|
| `HOST_NOT_INSTALLED` | `host-missing` | amber: setup to do |
| `HOST_UNAVAILABLE` | `host-unavailable` | red |
| `PROVIDER_NOT_FOUND` | `provider-missing` | amber: setup to do |
| `PROVIDER_NOT_AUTHENTICATED` | `provider-signed-out` | amber: setup to do |
| `PROVIDER_FAILED` | `provider-failed` | red |
| `REQUEST_TIMEOUT` | `timeout` | red |
| `REQUEST_CANCELLED` | `cancelled` | neutral: its state is `cancelled`, not `failed` |
| `CONTEXT_UNAVAILABLE` | `context-unavailable` | red |
| `INVALID_REQUEST` | `invalid-request` | red |
| `INTERNAL_ERROR`, or an unknown code | `internal-error` | red |

An error without a message gets its kind's own, which says what to do next without naming ports, hosts, or processes. Internal errors, and requests the host rejects as a protocol mismatch, add their request ID ("Reference: req_…"), which the host's diagnostics record. The popup has no Stop button yet (EXT-12), but a cancelled request already shows as stopped rather than failed.

## Browser context (CTX-01 through CTX-04)

The popup starts with **No context**. To attach context, click the **Selection** chip after selecting text on the page, or **This page** to extract readable text. The empty popup's starter suggestions (**Summarize this page**, **Explain my selection**) use the same capture and only fill in the question; nothing is sent until you ask. The popup shows the page title, sanitized URL, a text preview, and whether capture was truncated. Press **Ask** to send the question and the chosen context together; **No context** clears it. A fresh popup starts without context. An attached capture applies to that question only. Saved conversations retain only page title, sanitized URL, mode, and truncation metadata, never the raw captured text. No capture happens on popup open or a normal Ask.

Right-click selected text and choose **Ask Pervue about selection**, or right-click a page and choose **Ask Pervue about this page**. The menu opens the popup with that context previewed; it does not ask automatically. Chrome's menu selection includes text selected within frames. The handoff belongs to the clicked tab, expires after 30 seconds, and can be claimed once. If Chrome cannot open the action popup, the same composer opens in a tab with only an opaque handoff token in its URL. **Alt+Shift+P** (or **Command+Shift+P** on macOS) invokes the popup directly and focuses the input; Chrome users can change the shortcut under `chrome://extensions/shortcuts`.

The content script responds only to explicit capture messages. Selection supports regular text and focused text-field selections, up to 16 KiB of UTF-8. Page extraction prefers readable text from `main` or `article`, falls back to `body` when those are empty, skips scripts, navigation, hidden content, and form/editable surfaces, and stops at 64 KiB of UTF-8, 5,000 text nodes, or 512 Ki UTF-16 units examined in total. Direct popup capture is limited to the top frame; the menu selection comes from Chrome's click event. The service worker checks the response size again, gets the tab's title and URL, keeps at most 256 title code points and 2,048 URL bytes, removes URL credentials, query, and fragment, and rejects unsupported pages. Failed capture leaves Ask usable without context. Permission and unavailability outcomes use `CONTEXT_UNAVAILABLE`, and site access is used only for explicit popup/menu actions; the choice is not persisted. Page text is rendered as text, provider output through the HTML-free Markdown renderer, and callback errors omit raw content from logs.

### Trying it against the local host

For a user-facing macOS install, use the companion package paired with the extension's published ID. The setup page links to the package and provider guidance. Packaging and the clean-machine verification checklist live in `packaging/macos/README.md`.

For local development before installing a paired package, register a development build by hand:

1. Build the host: `cargo build -p pervue-host` in `native/`.
2. Load this directory unpacked and copy the extension ID from `chrome://extensions`.
3. Have the host print its manifest for that ID, and save it as `com.pervue.host.json` in Chrome's per-user `NativeMessagingHosts` directory, creating the directory if needed. For example, on Linux:

   ```bash
   native/target/debug/pervue-host --print-manifest <extension-id> \
     > ~/.config/google-chrome/NativeMessagingHosts/com.pervue.host.json
   ```

   The macOS directory is `~/Library/Application Support/Google/Chrome/NativeMessagingHosts/`. On Windows, save the file anywhere, then create the registry key `HKCU\Software\Google\Chrome\NativeMessagingHosts\com.pervue.host` and set its default value to the file's full path. The manifest allows only the IDs you pass, and its `path` is the host binary that printed it.
4. Open the popup. Its first line shows whether the companion app is found and Codex is installed and signed in; with Codex ready, ask anything. A host started by Chrome gets a minimal `PATH`, so install Codex where the host looks (see "Codex" in `native/README.md`), or set `PERVUE_PROVIDER_PATH` in the environment Chrome starts with.

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
