// Native Messaging's frame limit is recorded in the protocol contract (SEC-01).
// Browser-context limits below are extension policy and are not host limits.

/** Largest Native Messaging payload the host accepts, in UTF-8 bytes. */
export const MAX_NATIVE_MESSAGE_BYTES = 1024 * 1024;

/** Largest selected-text payload returned from a page (UTF-8 bytes). */
export const MAX_SELECTION_BYTES = 16 * 1024;

/** Largest readable-page payload returned from a page (UTF-8 bytes). */
export const MAX_PAGE_BYTES = 64 * 1024;

/** Maximum text nodes inspected while extracting readable page content. */
export const MAX_PAGE_TEXT_NODES = 5000;

/** @param {string} text */
export function utf8ByteLength(text) {
  return new TextEncoder().encode(text).byteLength;
}
