import assert from "node:assert/strict";
import { createConversationStore, CONVERSATIONS_KEY } from "../src/background/conversation-store.js";

let sequence = 0;
const uuid = () => "00000000-0000-4000-8000-" + String(++sequence).padStart(12, "0");
/** @type {Record<string, any>} */
const values = {};
/** @type {string[]} */
const reads = [];
const quota = 320 * 1024;
const storage = {
  /** @param {string} key */
  async get(key) { reads.push(key); return { [key]: structuredClone(values[key]) }; },
  /** @param {Record<string, any>} update */
  async set(update) {
    const next = { ...values, ...structuredClone(update) };
    if (new TextEncoder().encode(JSON.stringify(next)).byteLength > quota) {
      throw new Error("QUOTA_BYTES quota exceeded");
    }
    Object.assign(values, structuredClone(update));
  },
  /** @param {string} key */
  async remove(key) { delete values[key]; }
};
const store = createConversationStore(storage, uuid);
/** @type {string[]} */
const ids = [];
for (let i = 0; i < 5; i += 1) {
  const { id, assistantId } = await store.create({
    providerId: "codex", providerSessionId: "session", text: "q".repeat(150_000)
  });
  ids.push(id);
  await store.finish(id, assistantId, "a".repeat(150_000), []);
}
const recent = await store.list();
assert.equal(recent[0].id, ids.at(-1));
assert.ok(recent.length < 5, "quota eviction removes the oldest conversations");
assert.ok(!values["tabbeam.conversation." + ids[0]]);
assert.ok(values["tabbeam.conversation." + ids.at(-1)]);
assert.ok(JSON.stringify(values[CONVERSATIONS_KEY]).length < 4_000, "index stays small");
assert.ok((await store.get(ids.at(-1) ?? "")).messages[0].text.includes("[Stored excerpt truncated]"));
reads.length = 0;
await store.get(ids.at(-1) ?? "");
assert.equal(reads.length, 2, "opening one conversation reads only index and that record");
console.log("CON-02 quota eviction and bounded per-conversation storage passed");
