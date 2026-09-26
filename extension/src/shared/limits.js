// Trust-boundary limits shared with the native host are recorded in
// docs/protocol/native-messaging-v1.json (SEC-01).

/** Largest Native Messaging payload the host accepts, in UTF-8 bytes. */
export const MAX_NATIVE_MESSAGE_BYTES = 1024 * 1024;

/** Bounded native dialogue fallback (protocol v1 §5.1). */
export const MAX_HISTORY_MESSAGES = 32;
export const MAX_HISTORY_BYTES = 128 * 1024;

/** Largest selected-text payload returned from a page (UTF-8 bytes). */
export const MAX_SELECTION_BYTES = 16 * 1024;

/** Largest readable-page payload returned from a page (UTF-8 bytes). */
export const MAX_PAGE_BYTES = 64 * 1024;

/** Largest page title accepted in browser context, in UTF-8 bytes. */
export const MAX_CONTEXT_TITLE_BYTES = 1024;

/** Largest sanitized page URL accepted in browser context, in UTF-8 bytes. */
export const MAX_CONTEXT_URL_BYTES = 2048;

/** Maximum text nodes inspected while extracting readable page content. */
export const MAX_PAGE_TEXT_NODES = 5000;

/** Maximum source characters examined when extracting readable page text. */
export const MAX_PAGE_SCAN_CHARS = 512 * 1024;

/** @param {string} text */
export function utf8ByteLength(text) {
  return new TextEncoder().encode(text).byteLength;
}

/**
 * Keep a browser-provided selection within a UTF-8 byte limit. This also
 * repairs lone surrogates before JSON reaches the native host. The classic
 * content script mirrors this algorithm because MV3 injects it without ESM.
 * @param {string} value
 * @param {number} maxBytes
 */
export function boundedUtf8Text(value, maxBytes) {
  let bytes = 0;
  /** @type {string[]} */
  const characters = [];
  for (const character of value) {
    const codePoint = character.codePointAt(0) ?? 0;
    const width = codePoint <= 0x7f ? 1 : codePoint <= 0x7ff ? 2 : codePoint <= 0xffff ? 3 : 4;
    if (bytes + width > maxBytes) {
      return { text: characters.join(""), truncated: true };
    }
    bytes += width;
    characters.push(codePoint >= 0xd800 && codePoint <= 0xdfff ? "\ufffd" : character);
  }
  return { text: characters.join(""), truncated: false };
}
