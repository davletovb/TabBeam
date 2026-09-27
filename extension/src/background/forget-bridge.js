import { createRequestId } from "./ask-bridge.js";

/** Provider sessions still to forget, kept across service-worker restarts. */
export const PENDING_FORGETS_KEY = "pervue.pendingForgets";

/** How long the host gets to forget one conversation. */
export const FORGET_TIMEOUT_MS = 15_000;

/**
 * @typedef {{
 *   send(request: any, owner?: import("./native-connection.js").RequestOwner): void,
 *   forget(requestId: string): void
 * }} ForgetManager
 * @typedef {{provider_id: string, conversation_id: string, pervue_id?: string}} PendingForget
 * @typedef {"forgotten" | "failed" | "unreachable"} ForgetOutcome
 * @typedef {{
 *   schedule?: (callback: () => void, ms: number) => any,
 *   cancel?: (timer: any) => void
 * }} Timers
 */

/**
 * Asks the native host to forget one conversation (`conversation.forget`):
 * its provider-session mapping and the provider's own transcript. Resolves
 * `forgotten` when the host completed, `failed` when it answered with a
 * failure, and `unreachable` when the connection was lost, couldn't be
 * made, or no answer came within {@link FORGET_TIMEOUT_MS}.
 *
 * @param {{
 *   manager: ForgetManager,
 *   providerId: string,
 *   conversationId: string,
 *   createRequestId?: () => string,
 *   timers?: Timers
 * }} options
 * @returns {Promise<ForgetOutcome>}
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
    /** @param {ForgetOutcome} outcome */
    const settle = (outcome) => {
      if (settled) return;
      settled = true;
      if (timer !== null) cancel(timer);
      resolve(outcome);
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
            if (event?.event === "response.completed") settle("forgotten");
            else if (event?.event === "response.failed") {
              const reason = event.payload?.error?.reason;
              settle(reason === "HOST_PROTOCOL_MISMATCH" || reason === "HOST_READY_TIMEOUT"
                ? "unreachable" : "failed");
            }
          },
          onDisconnect: () => settle("unreachable")
        }
      );
    } catch {
      settle("unreachable");
    }
    if (!settled) {
      timer = schedule(() => {
        timer = null;
        manager.forget(requestId);
        settle("unreachable");
      }, FORGET_TIMEOUT_MS);
    }
  });
}

/** @param {any} entry @returns {entry is PendingForget} */
function isPendingForget(entry) {
  return typeof entry?.provider_id === "string" && entry.provider_id !== "" &&
    typeof entry?.conversation_id === "string" && entry.conversation_id !== "" &&
    (entry.pervue_id === undefined || typeof entry.pervue_id === "string");
}

/** @param {PendingForget} a @param {PendingForget} b */
function sameEntry(a, b) {
  return a.provider_id === b.provider_id && a.conversation_id === b.conversation_id;
}

/**
 * Forgets deleted conversations' provider sessions through a durable queue:
 * a session is recorded before the host is asked, and stays recorded until
 * the host confirms, so a deletion made while the companion app can't be
 * reached is finished by a later {@link flush}. Nothing is dropped until the
 * host confirms it. Storage updates run one at a time; asking the host runs
 * outside them, so recording never waits on it. A flush that can't reach the
 * host stops there rather than trying every session in turn.
 *
 * A session is recorded before its Pervue conversation is removed, so the
 * record is a tombstone: it is acted on only once `conversationExists` says
 * the conversation (`pervue_id`) is gone. Until then, or while that can't be
 * read, it waits: a deletion still in progress, or one that failed, never
 * costs a conversation that is still there its provider session.
 *
 * @param {{
 *   manager: ForgetManager,
 *   storage: {get(key: string): Promise<any>, set(values: object): Promise<void>},
 *   conversationExists?: (pervueId: string) => Promise<boolean>,
 *   createRequestId?: () => string,
 *   timers?: Timers
 * }} options
 */
export function createSessionForgetter(options) {
  const { manager, storage, conversationExists } = options;
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
      await storage.set({ [PENDING_FORGETS_KEY]: pending });
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
          if (entry.pervue_id !== undefined && conversationExists) {
            const removed = await conversationExists(entry.pervue_id).then((exists) => !exists, () => false);
            if (!removed) continue;
          }
          const outcome = await forgetProviderSession({
            manager,
            providerId: entry.provider_id,
            conversationId: entry.conversation_id,
            createRequestId: options.createRequestId,
            timers: options.timers
          });
          if (outcome === "forgotten") forgotten.push(entry);
          else if (outcome === "unreachable") {
            // The rest can't reach it either; they wait for the next flush.
            again = false;
            break;
          }
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
     * Records a provider session to forget. Resolves once it is stored, and
     * rejects when it couldn't be.
     * @param {string} providerId
     * @param {string} conversationId the host's conversation ID
     * @param {string} [pervueId] the Pervue conversation being deleted: the
     *   session is forgotten only once that conversation is gone
     */
    queue(providerId, conversationId, pervueId) {
      /** @type {PendingForget} */
      const entry = {
        provider_id: providerId,
        conversation_id: conversationId,
        ...(pervueId === undefined ? {} : { pervue_id: pervueId })
      };
      return update((pending) => [...pending.filter((other) => !sameEntry(other, entry)), entry]);
    },
    /** Asks the host to forget every recorded session; failures stay recorded. */
    flush
  };
}
