/** Provider-neutral conversation data; native session IDs are private metadata. */
export const CONVERSATION_SCHEMA_VERSION = 1;
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
  // Native protocol v1 accepts at most 32 messages and 128 KiB of history.
  const bounded = [];
  let bytes = 0;
  for (let index = history.length - 2; index >= 0 && bounded.length < 32; index -= 2) {
    const pair = history.slice(index, index + 2);
    const size = new TextEncoder().encode(pair[0].text + pair[1].text).length;
    if (bytes + size > 128 * 1024) break;
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
