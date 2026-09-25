import assert from "node:assert/strict";
import { serveConversationAskPort } from "../src/background/conversation-bridge.js";
import { createRequestId, hostDisconnectError, isExtensionPage } from "../src/background/ask-bridge.js";
import { RequestTooLargeError } from "../src/background/native-connection.js";
import { MockPort } from "./support/mock-port.mjs";

assert.equal(isExtensionPage({ url: "chrome-extension://test/src/popup/index.html" }, "chrome-extension://test/"), true);
assert.equal(isExtensionPage({ url: "https://example.com/" }, "chrome-extension://test/"), false);
assert.equal(new Set(Array.from({ length: 20 }, createRequestId)).size, 20);
assert.equal(hostDisconnectError("Specified native messaging host not found.").code, "HOST_NOT_INSTALLED");

async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 0));
  await new Promise((resolve) => setTimeout(resolve, 0));
}
const id = "conv_00000000-0000-4000-8000-000000000001";
/** @param {{send?: (request: any, owner: any) => void, setSession?: () => Promise<void>, getPrivate?: () => Promise<any>}} [options] */
function harness(options = {}) {
  const port = new MockPort("pervue.ask");
  /** @type {{request: any, owner: any}[]} */
  const sent = [];
  /** @type {any[]} */
  const finishes = [];
  const inFlight = new Set();
  const store = {
    async getPrivate() {
      if (options.getPrivate) return options.getPrivate();
      return {
        id, provider_id: "codex", provider_session_id: "host_session",
        messages: [{ role: "user", text: "Previous" }, { role: "assistant", text: "Answer", status: "complete" }]
      };
    },
    async begin() { return { assistantId: "msg_2" }; },
    async setSession() { if (options.setSession) await options.setSession(); },
    /** @param {...any} args */
    async finish(...args) { finishes.push(args); },
    async create() { return { id, assistantId: "msg_2" }; }
  };
  let nextId = 0;
  serveConversationAskPort(port, {
    store, inFlight, createRequestId: () => "req_test_" + ++nextId,
    manager: { send(request, owner) {
      sent.push({ request, owner });
      options.send?.(request, owner);
    } }
  });
  return { port, sent, finishes, inFlight };
}

{
  const { port, sent } = harness();
  port.emitMessage({ type: "ask", text: "Explain", context: { mode: "other" } });
  assert.equal(sent.length, 0);
  assert.equal(port.messages[0].payload.error.reason, "INVALID_PAYLOAD");
}
{
  const { port, sent } = harness({
    send() { throw new RequestTooLargeError(2 * 1024 * 1024); }
  });
  port.emitMessage({ type: "ask", text: "Huge question" });
  await settle();
  assert.equal(sent.length, 1);
  assert.equal(port.messages.at(-1).payload.error.reason, "REQUEST_TOO_LARGE");
}
{
  const { port, sent } = harness();
  port.emitMessage({ type: "ask", text: "Explain", context: {
    mode: "selection", text: "snippet", truncated: false,
    page: { title: "Page", url: "https://example.com/" }, extra: "never forwarded"
  } });
  await settle();
  assert.deepEqual(sent[0].request.payload.context, {
    mode: "selection", text: "snippet", truncated: false,
    page: { title: "Page", url: "https://example.com/" }
  });
  sent[0].owner.onDisconnect({ message: null });
  await settle();
  assert.equal(port.messages.at(-1).payload.error.reason, "HOST_DISCONNECTED");
}
{
  const { port, sent, finishes, inFlight } = harness({
    async setSession() { throw new Error("quota"); }
  });
  port.emitMessage({ type: "ask", text: "Follow-up", conversation_id: id });
  await settle();
  assert.equal(inFlight.has(id), true);
  sent[0].owner.onEvent({ event: "conversation.created", payload: { conversation_id: "new_session" } });
  await settle();
  assert.equal(finishes.length, 1);
  assert.equal(finishes[0][4].reason, "CONVERSATION_STORE_FAILED");
  assert.equal(sent[1].request.method, "request.cancel");
  assert.equal(inFlight.has(id), true, "the native turn still owns this conversation");
  sent[0].owner.onEvent({ event: "response.completed", payload: {} });
  await settle();
  assert.equal(finishes.length, 1, "late completion cannot replace the saved failure");
  assert.equal(inFlight.has(id), false);
}
{
  // EXT-12: a Stop message targets the in-flight native request and uses a
  // fresh request ID for the cancellation command.
  const { port, sent } = harness();
  port.emitMessage({ type: "ask", text: "Long answer" });
  await settle();
  assert.equal(sent.length, 1);
  assert.equal(sent[0].request.method, "conversation.send");
  const target = sent[0].request.request_id;
  port.emitMessage({ type: "cancel" });
  assert.equal(sent.length, 2);
  assert.equal(sent[1].request.method, "request.cancel");
  assert.equal(sent[1].request.payload.target_request_id, target);
  assert.ok(sent[1].request.request_id !== target);
}
{
  // Cancellation is remembered while a follow-up is loading from storage,
  // then sent immediately after the native request is registered.
  let release;
  const stored = new Promise((resolve) => { release = resolve; });
  const { port, sent } = harness({ getPrivate: () => stored });
  port.emitMessage({ type: "ask", text: "Follow-up", conversation_id: id });
  port.emitMessage({ type: "cancel" });
  assert.equal(sent.length, 0);
  release({
    id, provider_id: "codex", provider_session_id: "host_session",
    messages: [{ role: "user", text: "Previous" }, { role: "assistant", text: "Answer", status: "complete" }]
  });
  await settle();
  assert.equal(sent[0].request.method, "conversation.send");
  assert.equal(sent[1].request.method, "request.cancel");
  assert.equal(sent[1].request.payload.target_request_id, sent[0].request.request_id);
}
{
  const { port, sent, finishes } = harness({
    send(_request, owner) {
      owner.onDisconnect({ message: null });
      throw new Error("postMessage failed");
    }
  });
  port.emitMessage({ type: "ask", text: "Follow-up", conversation_id: id });
  await settle();
  assert.equal(sent.length, 1);
  assert.equal(finishes.length, 1, "disconnect and synchronous throw finish once");
  assert.equal(port.messages.filter((event) => event.event === "response.failed").length, 1);
}
console.log("CON-03 production bridge failure and lock tests passed");
