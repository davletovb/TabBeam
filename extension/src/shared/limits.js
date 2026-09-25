// Trust-boundary limits shared with the native host (SEC-01). The values are
// recorded in docs/protocol/native-messaging-v1.json, and tests on both sides
// keep them equal.

/** Largest Native Messaging payload the host accepts, in UTF-8 bytes. */
export const MAX_NATIVE_MESSAGE_BYTES = 1024 * 1024;

/** Largest selected-text payload returned from a page (UTF-8 bytes). */
export const MAX_SELECTION_BYTES = 16 * 1024;

/** @param {string} text */
export function utf8ByteLength(text) {
  return new TextEncoder().encode(text).byteLength;
}
