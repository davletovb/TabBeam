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
- dependency-free smoke validation.

Run the extension validation pipeline with:

```bash
cd extension
npm run lint
npm run typecheck
npm run build
npm test
```

The build emits an unpacked extension under `extension/dist/`. Lint and typecheck dependencies are pinned and fetched on demand by npm, so the repository does not need a generated dependency tree for this vanilla-JS foundation.

Provider/native messaging work intentionally begins in later tracker items.
