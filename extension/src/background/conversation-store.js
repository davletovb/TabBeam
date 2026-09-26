import {
  CONVERSATION_ID_PATTERN, CONVERSATION_SCHEMA_VERSION, contextMetadata,
  conversationTitle, publicConversation
} from "../shared/conversation-model.js";
import { boundedUtf8Text, utf8ByteLength } from "../shared/limits.js";

export const CONVERSATIONS_KEY = "pervue.conversations";
const RECORD_PREFIX = "pervue.conversation.";
const MAX_RECENT = 30;
// Chrome's default local-storage quota is 10 MiB. Leave room for settings and overhead.
const MAX_STORED_BYTES = 6 * 1024 * 1024;
const MAX_CONVERSATION_BYTES = 512 * 1024;
const MAX_MESSAGE_BYTES = 96 * 1024;
/** @typedef {{schema_version: number, recent_ids: string[], items: Record<string, any>, sizes: Record<string, number>}} ConversationIndex */

/** @param {string} id */
function recordKey(id) { return RECORD_PREFIX + id; }
/** @param {any} value */
function size(value) { return utf8ByteLength(JSON.stringify(value)); }
/** @param {string} value */
function storedText(value) {
  const result = boundedUtf8Text(value, MAX_MESSAGE_BYTES);
  return result.truncated ? result.text + "\n[Stored excerpt truncated]" : result.text;
}
/** @param {any} conversation */
function boundRecord(conversation) {
  for (const message of conversation.messages) {
    message.text = storedText(message.text);
    if (message.sources) {
      message.sources = message.sources.slice(-16).map((/** @type {any} */ source) =>
        size(source) <= 16 * 1024 ? source : { id: source.id, truncated: true });
    }
  }
  conversation.sources = conversation.sources.slice(-16).map((/** @type {any} */ source) =>
    size(source) <= 16 * 1024 ? source : { id: source.id, truncated: true });
  while (size(conversation) > MAX_CONVERSATION_BYTES && conversation.messages.length > 2) {
    conversation.messages.splice(0, 2);
    conversation.dropped_message_count = (conversation.dropped_message_count ?? 0) + 2;
  }
  return conversation;
}

/**
 * Serialized worker-owned persistence. Each conversation has a separate key,
 * so normal turns never read and rewrite every saved answer.
 * @param {{get(key: string): Promise<any>, set(values: object): Promise<void>, remove(key: string): Promise<void>}} storage
 * @param {() => string} [newId]
 * @param {() => string} [now]
 */
export function createConversationStore(storage, newId = () => crypto.randomUUID(), now = () => new Date().toISOString()) {
  let queue = Promise.resolve();
  /** @returns {Promise<ConversationIndex>} */
  async function readIndex() {
    const value = (await storage.get(CONVERSATIONS_KEY))[CONVERSATIONS_KEY];
    if (value === undefined) {
      return { schema_version: CONVERSATION_SCHEMA_VERSION, recent_ids: [], items: {}, sizes: {} };
    }
    if (value?.schema_version !== CONVERSATION_SCHEMA_VERSION ||
        !Array.isArray(value.recent_ids) || !value.items || !value.sizes ||
        typeof value.items !== "object" || typeof value.sizes !== "object") {
      throw new Error("Stored conversations need a compatible Pervue version.");
    }
    return /** @type {ConversationIndex} */ (value);
  }
  /** @param {ConversationIndex} index @param {string} id */
  async function readRecord(index, id) {
    if (!CONVERSATION_ID_PATTERN.test(id) || !index.items[id]) throw new Error("Conversation not found.");
    const value = (await storage.get(recordKey(id)))[recordKey(id)];
    if (!value) throw new Error("Conversation not found.");
    return value;
  }
  /** @param {ConversationIndex} index @param {string} protectedId */
  async function evictOldest(index, protectedId) {
    const victim = [...index.recent_ids].reverse().find((id) => id !== protectedId);
    if (!victim) return false;
    index.recent_ids = index.recent_ids.filter((id) => id !== victim);
    delete index.items[victim];
    delete index.sizes[victim];
    await storage.remove(recordKey(victim));
    return true;
  }
  /** @param {ConversationIndex} index @param {any} conversation */
  async function save(index, conversation) {
    boundRecord(conversation);
    const id = conversation.id;
    index.items[id] = {
      id, title: conversation.title, provider_id: conversation.provider_id,
      created_at: conversation.created_at, updated_at: conversation.updated_at
    };
    index.sizes[id] = size(conversation);
    index.recent_ids = [id, ...index.recent_ids.filter((old) => old !== id)];
    while (index.recent_ids.length > MAX_RECENT ||
        Object.values(index.sizes).reduce((total, value) => total + value, 0) > MAX_STORED_BYTES) {
      if (!await evictOldest(index, id)) break;
    }
    for (;;) {
      try {
        await storage.set({ [recordKey(id)]: conversation });
        await storage.set({ [CONVERSATIONS_KEY]: index });
        return;
      } catch (error) {
        if (!/quota|MAX_WRITE/i.test(String(error)) || !await evictOldest(index, id)) throw error;
      }
    }
  }
  /** @template T @param {(index: ConversationIndex) => Promise<T>} operation */
  function serialized(operation) {
    const task = queue.then(async () => operation(await readIndex()));
    queue = task.then(() => undefined, () => undefined);
    return task;
  }

  return {
    async list() {
      await queue;
      const index = await readIndex();
      return index.recent_ids.map((id) => index.items[id]).filter(Boolean);
    },
    /**
     * Deletes a conversation and its record.
     * @param {string} id
     */
    remove(id) {
      return serialized(async (index) => {
        await readRecord(index, id);
        index.recent_ids = index.recent_ids.filter((other) => other !== id);
        delete index.items[id];
        delete index.sizes[id];
        await storage.set({ [CONVERSATIONS_KEY]: index });
        await storage.remove(recordKey(id));
      });
    },
    /** @param {string} id */
    async get(id) {
      await queue;
      return publicConversation(await readRecord(await readIndex(), id));
    },
    /**
     * Whether the conversation exists. Throws when the stored conversations
     * can't be read, so an unreadable store is never taken for a deletion.
     * @param {string} id
     */
    async has(id) {
      await queue;
      return Object.hasOwn((await readIndex()).items, id);
    },
    /** @param {string} id */
    async getPrivate(id) {
      await queue;
      return structuredClone(await readRecord(await readIndex(), id));
    },
    /** @param {{providerId: string, providerSessionId: string, text: string, context?: any}} details */
    create({ providerId, providerSessionId, text, context }) {
      return serialized(async (index) => {
        const id = "conv_" + newId();
        const timestamp = now();
        const assistantId = "msg_" + newId();
        const conversation = {
          id, created_at: timestamp, updated_at: timestamp,
          title: conversationTitle(text), provider_id: providerId,
          provider_session_id: providerSessionId,
          messages: [
            { id: "msg_" + newId(), role: "user", text, timestamp, status: "complete" },
            { id: assistantId, role: "assistant", text: "", timestamp, status: "pending" }
          ],
          sources: [],
          ...(context ? { page_context_metadata: contextMetadata(context) } : {})
        };
        await save(index, conversation);
        return { id, assistantId };
      });
    },
    /** @param {string} id @param {string} text @param {any} [context] */
    begin(id, text, context) {
      return serialized(async (index) => {
        const conversation = await readRecord(index, id);
        const timestamp = now();
        for (const message of conversation.messages) {
          if (message.status === "pending") message.status = "failed";
        }
        const assistantId = "msg_" + newId();
        conversation.messages.push(
          { id: "msg_" + newId(), role: "user", text, timestamp, status: "complete" },
          { id: assistantId, role: "assistant", text: "", timestamp, status: "pending" }
        );
        if (context) conversation.page_context_metadata = contextMetadata(context);
        conversation.updated_at = timestamp;
        await save(index, conversation);
        return { assistantId };
      });
    },
    /** Retry the most recent failed pair without appending a duplicate user turn. */
    /** @param {string} id @param {string} text @param {any} [context] */
    retry(id, text, context) {
      return serialized(async (index) => {
        const conversation = await readRecord(index, id);
        const user = conversation.messages.at(-2);
        const assistant = conversation.messages.at(-1);
        if (
          user?.role !== "user" ||
          assistant?.role !== "assistant" ||
          assistant.status !== "failed" ||
          user.text !== text
        ) {
          throw new Error("Retry target unavailable.");
        }
        const timestamp = now();
        user.timestamp = timestamp;
        assistant.text = "";
        assistant.timestamp = timestamp;
        assistant.status = "pending";
        delete assistant.provider_metadata;
        delete assistant.sources;
        if (context) conversation.page_context_metadata = contextMetadata(context);
        conversation.updated_at = timestamp;
        await save(index, conversation);
        return { assistantId: assistant.id };
      });
    },
    /** Roll back a pending turn that was cancelled before native work started. */
    /** @param {string} id @param {string} assistantId @param {boolean} [preservePair] */
    discardPending(id, assistantId, preservePair = false) {
      return serialized(async (index) => {
        const conversation = await readRecord(index, id);
        const assistantIndex = conversation.messages.findIndex(
          (/** @type {any} */ item) => item.id === assistantId
        );
        const assistant = conversation.messages[assistantIndex];
        const user = conversation.messages[assistantIndex - 1];
        if (
          assistantIndex < 1 ||
          assistant?.role !== "assistant" ||
          assistant.status !== "pending" ||
          user?.role !== "user"
        ) {
          throw new Error("Pending turn not found.");
        }
        if (preservePair) {
          assistant.status = "failed";
          assistant.text = "";
          assistant.provider_metadata = {
            error: { code: "REQUEST_CANCELLED", reason: "USER_CANCELLED" }
          };
        } else {
          conversation.messages.splice(assistantIndex - 1, 2);
        }
        conversation.updated_at = now();
        await save(index, conversation);
      });
    },
    /** @param {string} id @param {string} providerSessionId */
    setSession(id, providerSessionId) {
      return serialized(async (index) => {
        const conversation = await readRecord(index, id);
        conversation.provider_session_id = providerSessionId;
        await save(index, conversation);
      });
    },
    /** @param {string} id @param {string} assistantId @param {string} text @param {any[]} sources @param {any} [error] */
    finish(id, assistantId, text, sources, error) {
      return serialized(async (index) => {
        const conversation = await readRecord(index, id);
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
        await save(index, conversation);
      });
    }
  };
}
