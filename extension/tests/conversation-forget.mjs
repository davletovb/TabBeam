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
  assert.equal(await done, true);

  const failed = forgetProviderSession({ manager, providerId: "codex", conversationId: "conv_2", createRequestId });
  manager.answer(1, "response.failed", { error: { code: "INTERNAL_ERROR", reason: "SESSION_FORGET_FAILED" } });
  assert.equal(await failed, false);

  const lost = forgetProviderSession({ manager, providerId: "codex", conversationId: "conv_3", createRequestId });
  manager.sent[2].owner.onDisconnect({ message: null });
  assert.equal(await lost, false);

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
  assert.equal(await silent, false);
  assert.deepEqual(manager.forgotten, [manager.sent[3].request.request_id], "a timed-out route is dropped");
}

// ---------- Deleting a conversation forgets its provider session ----------
{
  const storage = memoryStorage();
  let n = 0;
  const store = createConversationStore(storage, () => `00000000-0000-4000-8000-${String(++n).padStart(12, "0")}`);
  const manager = fakeManager();
  const forgetter = createSessionForgetter({ manager, storage, createRequestId });
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
    { provider_id: "claude", conversation_id: "conv_host_claude" }
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

console.log("Conversation forget tests passed");
