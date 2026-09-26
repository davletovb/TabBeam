import assert from "node:assert/strict";
import {
  CONVERSATIONS_DELETE_MESSAGE,
  answerConversationMessage
} from "../src/background/conversation-messages.js";
import { createConversationStore } from "../src/background/conversation-store.js";
import {
  FORGET_TIMEOUT_MS,
  PENDING_FORGETS_KEY,
  createSessionForgetter,
  forgetProviderSession
} from "../src/background/forget-bridge.js";

/** A native connection manager that records requests and lets the test answer them. */
function fakeManager() {
  /** @type {{request: any, owner: any}[]} */
  const sent = [];
  /** @type {string[]} */
  const forgotten = [];
  return {
    sent,
    forgotten,
    /** @param {any} request @param {any} owner */
    send(request, owner) { sent.push({ request, owner }); },
    /** @param {string} requestId */
    forget(requestId) { forgotten.push(requestId); },
    /** @param {number} index @param {string} event @param {any} [payload] */
    answer(index, event, payload = {}) {
      const { request, owner } = sent[index];
      owner.onEvent({ version: 1, type: "event", request_id: request.request_id, event, payload });
    }
  };
}

/** Storage backed by a plain object. */
function memoryStorage() {
  /** @type {Record<string, any>} */
  const saved = {};
  return {
    saved,
    /** @param {string} key */
    async get(key) { return key in saved ? { [key]: structuredClone(saved[key]) } : {}; },
    /** @param {any} values */
    async set(values) { Object.assign(saved, structuredClone(values)); },
    /** @param {string} key */
    async remove(key) { delete saved[key]; }
  };
}

async function settle() {
  for (let i = 0; i < 5; i += 1) await new Promise((resolve) => setTimeout(resolve, 0));
}

let nextId = 0;
const createRequestId = () => `req_forget_${++nextId}`;

// ---------- One forget request ----------
{
  const manager = fakeManager();
  const done = forgetProviderSession({ manager, providerId: "claude", conversationId: "conv_1", createRequestId });
  assert.deepEqual(manager.sent[0].request, {
    version: 1,
    type: "request",
    request_id: manager.sent[0].request.request_id,
    method: "conversation.forget",
    payload: { provider_id: "claude", conversation_id: "conv_1" }
  });
  manager.answer(0, "response.completed");
  assert.equal(await done, "forgotten");

  const failed = forgetProviderSession({ manager, providerId: "codex", conversationId: "conv_2", createRequestId });
  manager.answer(1, "response.failed", { error: { code: "INTERNAL_ERROR", reason: "SESSION_FORGET_FAILED" } });
  assert.equal(await failed, "failed");

  const lost = forgetProviderSession({ manager, providerId: "codex", conversationId: "conv_3", createRequestId });
  manager.sent[2].owner.onDisconnect({ message: null });
  assert.equal(await lost, "unreachable");

  /** @type {(() => void)[]} */
  const timers = [];
  const silent = forgetProviderSession({
    manager, providerId: "codex", conversationId: "conv_4", createRequestId,
    timers: {
      schedule(callback, ms) {
        assert.equal(ms, FORGET_TIMEOUT_MS);
        timers.push(callback);
        return timers.length;
      },
      cancel() {}
    }
  });
  timers[0]();
  assert.equal(await silent, "unreachable");
  assert.deepEqual(manager.forgotten, [manager.sent[3].request.request_id], "a timed-out route is dropped");
}

// ---------- Deleting a conversation forgets its provider session ----------
{
  const storage = memoryStorage();
  let n = 0;
  const store = createConversationStore(storage, () => `00000000-0000-4000-8000-${String(++n).padStart(12, "0")}`);
  const manager = fakeManager();
  const forgetter = createSessionForgetter({
    manager, storage, createRequestId, conversationExists: (id) => store.has(id)
  });
  const claude = await store.create({ providerId: "claude", providerSessionId: "conv_host_claude", text: "Hi" });
  const kept = await store.create({ providerId: "codex", providerSessionId: "conv_host_codex", text: "Keep" });

  // A running conversation is neither deleted nor forgotten.
  const busy = await answerConversationMessage(
    { type: CONVERSATIONS_DELETE_MESSAGE, conversation_id: claude.id }, store, new Set([claude.id]), forgetter);
  assert.equal(busy.ok, false);
  assert.equal(manager.sent.length, 0);

  const deleted = await answerConversationMessage(
    { type: CONVERSATIONS_DELETE_MESSAGE, conversation_id: claude.id }, store, new Set(), forgetter);
  assert.deepEqual(deleted, { ok: true });
  assert.deepEqual((await store.list()).map((item) => item.id), [kept.id]);
  // Recorded before the host is asked, so a lost answer is retried later.
  assert.deepEqual(storage.saved[PENDING_FORGETS_KEY], [
    { provider_id: "claude", conversation_id: "conv_host_claude", pervue_id: claude.id }
  ]);
  await settle();
  assert.equal(manager.sent.length, 1);
  assert.deepEqual(manager.sent[0].request.payload, { provider_id: "claude", conversation_id: "conv_host_claude" });

  // The companion app can't be reached: the session stays recorded ...
  manager.sent[0].owner.onDisconnect({ message: "Native host has exited." });
  await settle();
  assert.equal(storage.saved[PENDING_FORGETS_KEY].length, 1);

  // ... until a later flush (the next worker start) gets it forgotten.
  const retried = forgetter.flush();
  await settle();
  assert.equal(manager.sent.length, 2);
  manager.answer(1, "response.completed");
  await retried;
  assert.deepEqual(storage.saved[PENDING_FORGETS_KEY], []);

  // A deletion that fails forgets nothing.
  const missing = await answerConversationMessage(
    { type: CONVERSATIONS_DELETE_MESSAGE, conversation_id: claude.id }, store, new Set(), forgetter);
  assert.equal(missing.ok, false);
  await settle();
  assert.equal(manager.sent.length, 2);
  assert.deepEqual(storage.saved[PENDING_FORGETS_KEY], []);
}

// ---------- Recording never waits on the host ----------
{
  const storage = memoryStorage();
  const manager = fakeManager();
  const forgetter = createSessionForgetter({ manager, storage, createRequestId });
  await forgetter.queue("codex", "conv_a");
  const flushing = forgetter.flush();
  await settle();
  assert.equal(manager.sent.length, 1, "the first session is being forgotten");
  // A second deletion while the host still thinks about the first.
  await forgetter.queue("claude", "conv_b");
  assert.equal(storage.saved[PENDING_FORGETS_KEY].length, 2);
  void forgetter.flush();
  manager.answer(0, "response.completed");
  await settle();
  // The flush goes round again for the session recorded meanwhile.
  assert.equal(manager.sent.length, 2);
  assert.deepEqual(manager.sent[1].request.payload, { provider_id: "claude", conversation_id: "conv_b" });
  manager.answer(1, "response.completed");
  await flushing;
  await settle();
  assert.deepEqual(storage.saved[PENDING_FORGETS_KEY], []);
}

// ---------- Nothing unconfirmed is dropped; an unreachable host ends a flush ----------
{
  const storage = memoryStorage();
  const manager = fakeManager();
  const forgetter = createSessionForgetter({ manager, storage, createRequestId });
  for (let i = 0; i < 150; i += 1) await forgetter.queue("codex", `conv_${i}`);
  assert.equal(storage.saved[PENDING_FORGETS_KEY].length, 150, "no cap: every deletion is kept until confirmed");

  const flushing = forgetter.flush();
  await settle();
  assert.equal(manager.sent.length, 1);
  manager.sent[0].owner.onDisconnect({ message: "Specified native messaging host not found." });
  await flushing;
  assert.equal(manager.sent.length, 1, "the other 149 aren't tried against an unreachable host");
  assert.equal(storage.saved[PENDING_FORGETS_KEY].length, 150);

  // A host that refuses one session still gets asked about the rest.
  const retry = forgetter.flush();
  await settle();
  manager.answer(1, "response.failed", { error: { code: "INTERNAL_ERROR", reason: "SESSION_FORGET_FAILED" } });
  await settle();
  assert.equal(manager.sent.length, 3);
  for (let index = 2; index < 151; index += 1) {
    manager.answer(index, "response.completed");
    await settle();
  }
  await retry;
  assert.deepEqual(storage.saved[PENDING_FORGETS_KEY], [{ provider_id: "codex", conversation_id: "conv_0" }]);
}

// ---------- Deleting survives failure at every step ----------
{
  const storage = memoryStorage();
  let n = 100;
  const store = createConversationStore(storage, () => `00000000-0000-4000-8000-${String(++n).padStart(12, "0")}`);
  // The pending-forget record lives in its own storage here, so its writes can fail alone.
  const pendingStorage = memoryStorage();
  let pendingWritesFail = false;
  const flakyPending = {
    get: pendingStorage.get,
    /** @param {any} values */
    async set(values) {
      if (pendingWritesFail) throw new Error("QUOTA_BYTES quota exceeded");
      await pendingStorage.set(values);
    }
  };
  const manager = fakeManager();
  const forgetter = createSessionForgetter({
    manager, storage: flakyPending, createRequestId, conversationExists: (id) => store.has(id)
  });
  const conversation = await store.create({ providerId: "claude", providerSessionId: "conv_host_1", text: "Hi" });

  // 1. The tombstone can't be stored: nothing is deleted, the session stays reachable.
  pendingWritesFail = true;
  const refused = await answerConversationMessage(
    { type: CONVERSATIONS_DELETE_MESSAGE, conversation_id: conversation.id }, store, new Set(), forgetter);
  assert.equal(refused.ok, false);
  assert.equal(await store.has(conversation.id), true);
  assert.equal((await store.getPrivate(conversation.id)).provider_session_id, "conv_host_1");
  await settle();
  assert.equal(manager.sent.length, 0);
  pendingWritesFail = false;

  // 2. The tombstone is stored but the removal fails: the conversation stays,
  //    and its provider session is not forgotten underneath it.
  const failingStore = { ...store, async remove() { throw new Error("storage unavailable"); } };
  const notRemoved = await answerConversationMessage(
    { type: CONVERSATIONS_DELETE_MESSAGE, conversation_id: conversation.id }, failingStore, new Set(), forgetter);
  assert.equal(notRemoved.ok, false);
  await forgetter.flush();
  await settle();
  assert.equal(manager.sent.length, 0, "a live conversation's session is never forgotten");
  assert.equal(pendingStorage.saved[PENDING_FORGETS_KEY].length, 1, "the tombstone waits");

  // 3. Removed, then the worker stops before asking the host: the next flush finishes it.
  await store.remove(conversation.id);
  const finishing = forgetter.flush();
  await settle();
  assert.equal(manager.sent.length, 1);
  assert.deepEqual(manager.sent[0].request.payload, { provider_id: "claude", conversation_id: "conv_host_1" });
  manager.answer(0, "response.completed");
  await finishing;
  assert.deepEqual(pendingStorage.saved[PENDING_FORGETS_KEY], []);
}

console.log("Conversation forget tests passed");
