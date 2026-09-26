/** Lists saved conversations: `{value: ConversationSummary[]}`. */
export const CONVERSATIONS_LIST_MESSAGE = "pervue.conversations.list";
/** Reads one conversation: `{conversation_id}` → `{value: Conversation}`. */
export const CONVERSATIONS_GET_MESSAGE = "pervue.conversations.get";
/** Deletes one conversation: `{conversation_id}` → `{}`. */
export const CONVERSATIONS_DELETE_MESSAGE = "pervue.conversations.delete";

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
 * @param {{queue(providerId: string, conversationId: string): Promise<void>, flush(): Promise<void>}} [forgetter]
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
 * Removes a conversation, then records its provider session with
 * `forgetter` and has it forgotten in the background. The conversation goes
 * first: a removal that fails must not cost a conversation that is still
 * there its provider session.
 *
 * @param {string} id
 * @param {{getPrivate(id: string): Promise<any>, remove(id: string): Promise<void>}} store
 * @param {{queue(providerId: string, conversationId: string): Promise<void>, flush(): Promise<void>}} [forgetter]
 */
async function deleteConversation(id, store, forgetter) {
  const record = forgetter ? await store.getPrivate(id).catch(() => null) : null;
  await store.remove(id);
  const providerId = record?.provider_id;
  const sessionId = record?.provider_session_id;
  if (!forgetter || typeof providerId !== "string" || typeof sessionId !== "string" ||
      providerId === "" || sessionId === "") {
    return;
  }
  // Deleted either way: a session that can't be recorded is left behind.
  await forgetter.queue(providerId, sessionId).catch(() => {});
  void forgetter.flush();
}
