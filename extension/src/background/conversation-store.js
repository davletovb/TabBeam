import {
  CONVERSATION_ID_PATTERN,
  CONVERSATION_SCHEMA_VERSION,
  contextMetadata,
  conversationTitle,
  publicConversation
} from "../shared/conversation-model.js";

export const CONVERSATIONS_KEY = "pervue.conversations";
const MAX_RECENT = 30;
/** @typedef {{schema_version: number, recent_ids: string[], conversations: Record<string, any>}} ConversationState */

/**
 * Storage runs only in the worker. Writes are serialized so two UI surfaces
 * cannot overwrite each other's recent index or conversation updates.
 * @param {{get(key: string): Promise<any>, set(values: object): Promise<void>}} storage
 * @param {() => string} [newId]
 * @param {() => string} [now]
 */
export function createConversationStore(storage, newId = () => crypto.randomUUID(), now = () => new Date().toISOString()) {
  let queue = Promise.resolve();

  /** @returns {Promise<ConversationState>} */
  async function read() {
    const value = (await storage.get(CONVERSATIONS_KEY))[CONVERSATIONS_KEY];
    if (value === undefined) {
      return { schema_version: CONVERSATION_SCHEMA_VERSION, recent_ids: [], conversations: {} };
    }
    if (value?.schema_version !== CONVERSATION_SCHEMA_VERSION ||
        !Array.isArray(value.recent_ids) || !value.conversations ||
        typeof value.conversations !== "object" || Array.isArray(value.conversations)) {
      throw new Error("Stored conversations need a compatible Pervue version.");
    }
    return /** @type {ConversationState} */ (value);
  }

  /** @template T @param {(state: ConversationState) => T} change @returns {Promise<T>} */
  function write(change) {
    const task = queue.then(async () => {
      const state = await read();
      const result = change(state);
      await storage.set({ [CONVERSATIONS_KEY]: state });
      return result;
    });
    queue = task.then(() => undefined, () => undefined);
    return task;
  }

  /** @param {ConversationState} state @param {string} id */
  function bump(state, id) {
    state.recent_ids = [id, ...state.recent_ids.filter((existing) => existing !== id)].slice(0, MAX_RECENT);
    for (const old of Object.keys(state.conversations)) {
      if (!state.recent_ids.includes(old)) delete state.conversations[old];
    }
  }

  /** @param {ConversationState} state @param {string} id */
  function requireConversation(state, id) {
    if (!CONVERSATION_ID_PATTERN.test(id) || !state.conversations[id]) {
      throw new Error("Conversation not found.");
    }
    return state.conversations[id];
  }

  return {
    async list() {
      await queue;
      const state = await read();
      return state.recent_ids.map((id) => state.conversations[id]).filter(Boolean).map((conversation) => ({
        id: conversation.id, title: conversation.title, provider_id: conversation.provider_id,
        created_at: conversation.created_at, updated_at: conversation.updated_at
      }));
    },
    /** @param {string} id */
    async get(id) {
      await queue;
      const state = await read();
      return publicConversation(requireConversation(state, id));
    },
    /** Native metadata stays in the worker. */
    /** @param {string} id */
    async getPrivate(id) {
      await queue;
      const state = await read();
      return structuredClone(requireConversation(state, id));
    },
    /** @param {{providerId: string, providerSessionId: string, text: string, context?: any}} details */
    create({ providerId, providerSessionId, text, context }) {
      return write((state) => {
        const id = `conv_${newId()}`;
        const timestamp = now();
        const assistantId = `msg_${newId()}`;
        state.conversations[id] = {
          id, created_at: timestamp, updated_at: timestamp,
          title: conversationTitle(text), provider_id: providerId,
          provider_session_id: providerSessionId,
          messages: [
            { id: `msg_${newId()}`, role: "user", text, timestamp, status: "complete" },
            { id: assistantId, role: "assistant", text: "", timestamp, status: "pending" }
          ],
          sources: [],
          ...(context ? { page_context_metadata: contextMetadata(context) } : {})
        };
        bump(state, id);
        return { id, assistantId };
      });
    },
    /** @param {string} id @param {string} text @param {any} [context] */
    begin(id, text, context) {
      return write((state) => {
        const conversation = requireConversation(state, id);
        const timestamp = now();
        // After a worker restart an abandoned response cannot remain pending.
        for (const message of conversation.messages) {
          if (message.status === "pending") message.status = "failed";
        }
        const assistantId = `msg_${newId()}`;
        conversation.messages.push(
          { id: `msg_${newId()}`, role: "user", text, timestamp, status: "complete" },
          { id: assistantId, role: "assistant", text: "", timestamp, status: "pending" }
        );
        if (context) conversation.page_context_metadata = contextMetadata(context);
        conversation.updated_at = timestamp;
        bump(state, id);
        return { assistantId };
      });
    },
    /** @param {string} id @param {string} providerSessionId */
    setSession(id, providerSessionId) {
      return write((state) => {
        requireConversation(state, id).provider_session_id = providerSessionId;
      });
    },
    /** @param {string} id @param {string} assistantId @param {string} text @param {any[]} sources @param {any} [error] */
    finish(id, assistantId, text, sources, error) {
      return write((state) => {
        const conversation = requireConversation(state, id);
        const message = conversation.messages.find((/** @type {any} */ item) => item.id === assistantId);
        if (!message) throw new Error("Response message not found.");
        message.text = text;
        message.status = error ? "failed" : "complete";
        if (error) message.provider_metadata = { error: { code: error.code, reason: error.reason } };
        if (sources.length) {
          message.sources = sources;
          conversation.sources.push(...sources);
        }
        conversation.updated_at = now();
        bump(state, id);
      });
    }
  };
}
