import { MAX_HISTORY_BYTES, MAX_HISTORY_MESSAGES, utf8ByteLength, boundedUtf8Text } from "./limits.js";

/** Provider-neutral conversation data; native session IDs are private metadata. */
export const CONVERSATION_SCHEMA_VERSION = 2;
export const CONVERSATION_ID_PATTERN = /^conv_[0-9a-f-]{36}$/;

/** @param {string} text */
export function conversationTitle(text) {
  return [...text.trim().replace(/\s+/gu, " ")].slice(0, 80).join("");
}

/** @param {any} context */
export function contextMetadata(context) {
  if (!context) return undefined;
  return {
    mode: context.mode,
    truncated: context.truncated === true,
    page: { title: context.page.title, url: context.page.url }
  };
}

/** @param {any} conversation Only complete pairs are usable as dialogue. */
export function dialogueHistory(conversation) {
  const history = [];
  for (let index = 0; index + 1 < conversation.messages.length; index += 2) {
    const user = conversation.messages[index];
    const assistant = conversation.messages[index + 1];
    if (user?.role === "user" && assistant?.role === "assistant" &&
        assistant.status === "complete" && assistant.text.trim()) {
      history.push({ role: "user", text: user.text });
      history.push({ role: "assistant", text: assistant.text });
    }
  }
  // Keep whole pairs. If the newest answer is too large, retain an excerpt
  // rather than returning no context at all.
  const bounded = [];
  let bytes = 0;
  for (let index = history.length - 2; index >= 0 && bounded.length < MAX_HISTORY_MESSAGES; index -= 2) {
    let pair = history.slice(index, index + 2);
    let size = utf8ByteLength(pair[0].text) + utf8ByteLength(pair[1].text);
    if (size > MAX_HISTORY_BYTES && bounded.length === 0) {
      const user = boundedUtf8Text(pair[0].text, MAX_HISTORY_BYTES / 4).text;
      const assistant = boundedUtf8Text(pair[1].text, MAX_HISTORY_BYTES - utf8ByteLength(user)).text;
      pair = [{ role: "user", text: user }, { role: "assistant", text: assistant }];
      size = utf8ByteLength(user) + utf8ByteLength(assistant);
    }
    if (bytes + size > MAX_HISTORY_BYTES) break;
    bounded.unshift(...pair);
    bytes += size;
  }
  return bounded;
}

/** @param {any} conversation Provider session metadata stays in the worker. */
export function publicConversation(conversation) {
  if (!conversation) return null;
  const visible = structuredClone(conversation);
  delete visible.provider_session_id;
  return visible;
}
