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
- HTTP/HTTPS content-script scaffold;
- keyboard-command scaffold;
- context-menu scaffold;
- service-worker-owned Native Messaging connection manager;
- dependency-free smoke/lifecycle validation.

## Native Messaging connection lifecycle

`src/background/native-connection.js` owns the browser-side native port lifecycle.

- The native port opens lazily on the first request.
- One module-scope manager instance lives in the service worker, so popup closure does not own or tear down the native connection.
- In-flight requests are multiplexed by protocol `request_id`. `send()` rejects IDs outside the protocol v1 grammar before opening the port, because the host could not echo them back to the right request.
- Terminal protocol events release their request route before the owner's handler runs, so the owner can reuse the request ID or disconnect.
- Native-port disconnect clears in-flight routes and notifies every request owner.
- Requester and listener callbacks are isolated: a callback that throws is reported (by default with `console.error`) and cannot stop other owners from being notified or the native port from closing.
- A subsequent request reconnects automatically.
- Stale callbacks from an old port are ignored after a replacement connection is established.

The canonical Native Messaging host name is currently `com.pervue.host`. Packaging/registration work later in the tracker must register the companion under that same name.

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
