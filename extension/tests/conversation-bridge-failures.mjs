import assert from "node:assert/strict";
import { serveConversationAskPort } from "../src/background/conversation-bridge.js";
import { RequestTooLargeError } from "../src/background/native-connection.js";
import { MockPort } from "./support/mock-port.mjs";

async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 0));
  await new Promise((resolve) => setTimeout(resolve, 0));
}
const id = "conv_00000000-0000-4000-8000-000000000001";
/** @param {{send?: (request: any, owner: any) => void, setSession?: () => Promise<void>}} [options] */
function harness(options = {}) {
  const port = new MockPort("pervue.ask");
  /** @type {{request: any, owner: any}[]} */
  const sent = [];
  /** @type {any[]} */
  const finishes = [];
  const inFlight = new Set();
  const store = {
    async getPrivate() { return {
      id, provider_id: "codex", provider_session_id: "host_session",
      messages: [{ role: "user", text: "Previous" }, { role: "assistant", text: "Answer", status: "complete" }]
    }; },
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
