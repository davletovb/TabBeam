/** Lists saved conversations: `{value: ConversationSummary[]}`. */
export const CONVERSATIONS_LIST_MESSAGE = "tabbeam.conversations.list";
/** Reads one conversation: `{conversation_id}` → `{value: Conversation}`. */
export const CONVERSATIONS_GET_MESSAGE = "tabbeam.conversations.get";
/** Deletes one conversation: `{conversation_id}` → `{}`. */
export const CONVERSATIONS_DELETE_MESSAGE = "tabbeam.conversations.delete";

const MESSAGES = new Set([CONVERSATIONS_LIST_MESSAGE, CONVERSATIONS_GET_MESSAGE, CONVERSATIONS_DELETE_MESSAGE]);

/** @param {any} message */
export function isConversationMessage(message) {
  return MESSAGES.has(message?.type);
}

/**
 * Answers the extension pages' conversation requests. A conversation with a
 * question still running can't be deleted: its answer would have nowhere to
 * go. Deleting one also has the companion app forget its provider session
 * and the provider's own transcript (see {@link deleteConversation}).
 *
 * @param {any} message
 * @param {{
 *   list(): Promise<any[]>,
 *   get(id: string): Promise<any>,
 *   getPrivate(id: string): Promise<any>,
 *   remove(id: string): Promise<void>
 * }} store
 * @param {Set<string>} inFlight conversation IDs with a question running
 * @param {{queue(providerId: string, conversationId: string, tabbeamId?: string): Promise<void>, flush(): Promise<void>}} [forgetter]
 * @returns {Promise<{ok: true, value?: any} | {ok: false, error: string}>}
 */
export async function answerConversationMessage(message, store, inFlight, forgetter) {
  try {
    switch (message.type) {
      case CONVERSATIONS_LIST_MESSAGE:
        return { ok: true, value: await store.list() };
      case CONVERSATIONS_GET_MESSAGE:
        return { ok: true, value: await store.get(message.conversation_id) };
      case CONVERSATIONS_DELETE_MESSAGE:
        if (inFlight.has(message.conversation_id)) {
          return { ok: false, error: "Stop or finish the answer in progress, then delete the conversation." };
        }
        await deleteConversation(message.conversation_id, store, forgetter);
        return { ok: true };
      default:
        return { ok: false, error: "Unknown request." };
    }
  } catch {
    return {
      ok: false,
      error: message.type === CONVERSATIONS_DELETE_MESSAGE
        ? "Couldn't delete the conversation."
        : "Conversation history unavailable."
    };
  }
}

/**
 * Removes a conversation and has the companion app forget its provider
 * session, in an order that survives failure at any step:
 *
 * 1. the session is recorded with `forgetter` as a tombstone for this
 *    conversation; if that can't be stored, nothing is deleted;
 * 2. the conversation is removed; if that fails, the tombstone waits, because
 *    `forgetter` only acts on tombstones whose conversation is gone;
 * 3. the session is forgotten in the background, and retried until the host
 *    confirms.
 *
 * @param {string} id
 * @param {{getPrivate(id: string): Promise<any>, remove(id: string): Promise<void>}} store
 * @param {{queue(providerId: string, conversationId: string, tabbeamId?: string): Promise<void>, flush(): Promise<void>}} [forgetter]
 */
async function deleteConversation(id, store, forgetter) {
  if (!forgetter) {
    await store.remove(id);
    return;
  }
  const record = await store.getPrivate(id);
  const providerId = record?.provider_id;
  const sessionId = record?.provider_session_id;
  const hasSession = typeof providerId === "string" && typeof sessionId === "string" &&
    providerId !== "" && sessionId !== "";
  if (hasSession) await forgetter.queue(providerId, sessionId, id);
  await store.remove(id);
  if (hasSession) void forgetter.flush();
}
