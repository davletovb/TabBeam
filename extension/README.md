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

Run the current checks with:

```bash
cd extension
npm test
```

Provider/native messaging work intentionally begins in later tracker items.
