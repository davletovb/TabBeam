import { createRequestId } from "./ask-bridge.js";

/** Provider sessions still to forget, kept across service-worker restarts. */
export const PENDING_FORGETS_KEY = "pervue.pendingForgets";

/** How long the host gets to forget one conversation. */
export const FORGET_TIMEOUT_MS = 15_000;

/** The most sessions kept waiting to be forgotten; the oldest go first. */
const MAX_PENDING = 100;

/**
 * @typedef {{
 *   send(request: any, owner?: import("./native-connection.js").RequestOwner): void,
 *   forget(requestId: string): void
 * }} ForgetManager
 * @typedef {{provider_id: string, conversation_id: string}} PendingForget
 * @typedef {{
 *   schedule?: (callback: () => void, ms: number) => any,
 *   cancel?: (timer: any) => void
 * }} Timers
 */

/**
 * Asks the native host to forget one conversation (`conversation.forget`):
 * its provider-session mapping and the provider's own transcript. Resolves
 * whether the host completed; a failure, a lost connection, or no answer
 * within {@link FORGET_TIMEOUT_MS} resolves false.
 *
 * @param {{
 *   manager: ForgetManager,
 *   providerId: string,
 *   conversationId: string,
 *   createRequestId?: () => string,
 *   timers?: Timers
 * }} options
 * @returns {Promise<boolean>}
 */
export function forgetProviderSession(options) {
  const { manager, providerId, conversationId, timers = {} } = options;
  const schedule = timers.schedule ?? setTimeout;
  const cancel = timers.cancel ?? clearTimeout;
  const requestId = (options.createRequestId ?? createRequestId)();
  return new Promise((resolve) => {
    let settled = false;
    /** @type {any} */
    let timer = null;
    /** @param {boolean} forgotten */
    const settle = (forgotten) => {
      if (settled) return;
      settled = true;
      if (timer !== null) cancel(timer);
      resolve(forgotten);
    };
    try {
      manager.send(
        {
          version: 1,
          type: "request",
          request_id: requestId,
          method: "conversation.forget",
          payload: { provider_id: providerId, conversation_id: conversationId }
        },
        {
          onEvent(event) {
            if (event?.event === "response.completed") settle(true);
            else if (event?.event === "response.failed") settle(false);
          },
          onDisconnect: () => settle(false)
        }
      );
    } catch {
      settle(false);
    }
    if (!settled) {
      timer = schedule(() => {
        timer = null;
        manager.forget(requestId);
        settle(false);
      }, FORGET_TIMEOUT_MS);
    }
  });
}

/** @param {any} entry @returns {entry is PendingForget} */
function isPendingForget(entry) {
  return typeof entry?.provider_id === "string" && entry.provider_id !== "" &&
    typeof entry?.conversation_id === "string" && entry.conversation_id !== "";
}

/** @param {PendingForget} a @param {PendingForget} b */
function sameEntry(a, b) {
  return a.provider_id === b.provider_id && a.conversation_id === b.conversation_id;
}

/**
 * Forgets deleted conversations' provider sessions through a durable queue:
 * a session is recorded before the host is asked, and stays recorded until
 * the host confirms, so a deletion made while the companion app can't be
 * reached is finished by a later {@link flush}. Storage updates run one at a
 * time; asking the host runs outside them, so recording never waits on it.
 *
 * @param {{
 *   manager: ForgetManager,
 *   storage: {get(key: string): Promise<any>, set(values: object): Promise<void>},
 *   createRequestId?: () => string,
 *   timers?: Timers
 * }} options
 */
export function createSessionForgetter(options) {
  const { manager, storage } = options;
  /** @type {Promise<void>} */
  let updates = Promise.resolve();
  /** @type {Promise<void> | null} */
  let flushing = null;
  let again = false;

  /** @returns {Promise<PendingForget[]>} */
  async function load() {
    const stored = (await storage.get(PENDING_FORGETS_KEY))?.[PENDING_FORGETS_KEY];
    return Array.isArray(stored) ? stored.filter(isPendingForget) : [];
  }

  /** @param {(pending: PendingForget[]) => PendingForget[]} change */
  function update(change) {
    const next = updates.then(async () => {
      const pending = change(await load());
      await storage.set({ [PENDING_FORGETS_KEY]: pending.slice(-MAX_PENDING) });
    });
    updates = next.catch(() => {});
    return next;
  }

  /** @returns {Promise<void>} */
  function flush() {
    if (flushing) {
      // Recorded after this flush read the queue: go round once more.
      again = true;
      return flushing;
    }
    flushing = (async () => {
      do {
        again = false;
        await updates;
        const pending = await load().catch(() => []);
        /** @type {PendingForget[]} */
        const forgotten = [];
        for (const entry of pending) {
          const done = await forgetProviderSession({
            manager,
            providerId: entry.provider_id,
            conversationId: entry.conversation_id,
            createRequestId: options.createRequestId,
            timers: options.timers
          });
          if (done) forgotten.push(entry);
        }
        if (forgotten.length > 0) {
          await update((current) =>
            current.filter((entry) => !forgotten.some((done) => sameEntry(done, entry))));
        }
      } while (again);
    })().catch(() => {}).finally(() => {
      flushing = null;
      if (again) void flush();
    });
    return flushing;
  }

  return {
    /**
     * Records a provider session to forget. Resolves once it is stored.
     * @param {string} providerId
     * @param {string} conversationId the host's conversation ID
     */
    queue(providerId, conversationId) {
      const entry = { provider_id: providerId, conversation_id: conversationId };
      return update((pending) => [...pending.filter((other) => !sameEntry(other, entry)), entry]);
    },
    /** Asks the host to forget every recorded session; failures stay recorded. */
    flush
  };
}
