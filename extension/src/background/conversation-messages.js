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
 * go.
 *
 * @param {any} message
 * @param {{list(): Promise<any[]>, get(id: string): Promise<any>, remove(id: string): Promise<void>}} store
 * @param {Set<string>} inFlight conversation IDs with a question running
 * @returns {Promise<{ok: true, value?: any} | {ok: false, error: string}>}
 */
export async function answerConversationMessage(message, store, inFlight) {
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
        await store.remove(message.conversation_id);
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
