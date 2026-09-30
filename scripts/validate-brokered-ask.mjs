#!/usr/bin/env node
// Sends real asks through the built host and a real Seatline broker.
//
// The other round trips use the host's `fake` scaffold, which never reaches the
// broker, so they cannot see what the broker does with a request. This one runs
// a fake `codex`, authorizes TabBeam in a real broker, and checks the two things
// TabBeam depends on:
//
//   * a plain question (no page context) asks for the provider's own tool
//     settings, and completes once the grant allows that;
//   * without the grant it fails with PROVIDER_DEFAULT_TOOLS_DENIED and a message
//     that says what to do, while an ask that carries page context still works.
//
// Linux and macOS only (the broker listens on a Unix socket).
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { frameNativeMessage, parseNativeFrames } from "./protocol-support.mjs";

const { values } = parseArgs({ options: { companion: { type: "string" }, host: { type: "string" } } });
if (!values.companion || !values.host) {
  console.error("usage: validate-brokered-ask.mjs --companion <seatline-companion> --host <tabbeam-host>");
  process.exit(2);
}
const companion = resolve(values.companion);
const host = resolve(values.host);
const ORIGIN = "chrome-extension://abcdefghijklmnopabcdefghijklmnop/";
const TERMINAL = new Set(["response.completed", "response.failed"]);

// Provider workspaces need trusted ancestors, and a Unix socket path must be
// short, so the scratch directory lives under the working directory.
const root = mkdtempSync(join(process.cwd(), ".brokered-ask-"));
const providers = join(root, "providers");
mkdirSync(providers);
const codex = join(providers, "codex");
writeFileSync(codex, `#!/bin/sh
if [ "$1" = "--version" ]; then echo 'codex-cli 0.1.0'; exit 0; fi
if [ "$1" = "login" ]; then echo 'Logged in using ChatGPT'; exit 0; fi
cat >/dev/null
echo '{"type":"thread.started","thread_id":"test-session"}'
echo '{"type":"turn.started"}'
echo '{"type":"item.completed","item":{"id":"message-1","type":"agent_message","text":"Brokered answer"}}'
echo '{"type":"turn.completed","usage":{"input_tokens":2,"output_tokens":3}}'
`);
chmodSync(codex, 0o700);

const env = {
  ...process.env,
  HOME: root,
  XDG_CACHE_HOME: join(root, "cache"),
  XDG_DATA_HOME: join(root, "data"),
  SEATLINE_DATA_DIR: join(root, "seatline"),
  TABBEAM_PROVIDER_PATH: providers,
};
for (const name of ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"]) delete env[name];

const authorize = (...extra) => execFileSync(companion,
  ["authorize", "tabbeam", "codex", "--cache-title=TabBeam", ...extra], { env, stdio: "ignore" });

/** Runs one request through a fresh host process and returns its frames. */
async function ask(request, timeoutMs = 30_000) {
  const child = spawn(host, [ORIGIN], { env, stdio: ["pipe", "pipe", "pipe"] });
  child.stdin.on("error", () => {});
  let stderr = ""; child.stderr.on("data", chunk => { stderr += chunk; });
  const closed = once(child, "close");
  const timer = setTimeout(() => child.kill(), timeoutMs);
  try {
    child.stdin.write(frameNativeMessage(Buffer.from(JSON.stringify({ version: 1, type: "request", ...request }))));
    let output = Buffer.alloc(0);
    for await (const chunk of child.stdout) {
      output = Buffer.concat([output, chunk]);
      let frames;
      try { frames = parseNativeFrames(output); } catch { continue; }
      if (frames.some(frame => frame.request_id === request.request_id && TERMINAL.has(frame.event))) {
        // Keep the port open until the request ends: EOF cancels unfinished work.
        child.stdin.end();
        await closed;
        return frames.filter(frame => frame.request_id === request.request_id);
      }
    }
    await closed;
    throw new Error(`the host ended without answering ${request.request_id}\n${stderr}`);
  } finally {
    clearTimeout(timer);
    child.kill();
  }
}

const terminal = frames => frames.find(frame => TERMINAL.has(frame.event));
const plainAsk = id => ({ request_id: id, method: "conversation.send", payload: { provider_id: "codex", input: { text: "Hello" } } });
const contextAsk = id => ({
  request_id: id,
  method: "conversation.send",
  payload: {
    provider_id: "codex",
    input: { text: "Explain this" },
    context: { mode: "selection", text: "Selected reference text", truncated: false, page: { title: "Example", url: "https://example.com/a" } },
  },
});

authorize(ORIGIN, "--allow-provider-default");
const broker = spawn(companion, ["serve"], { env, stdio: "ignore" });
try {
  for (let i = 0; i < 100 && !existsSync(join(env.SEATLINE_DATA_DIR, "broker.sock")); i++) await new Promise(resolve => setTimeout(resolve, 50));
  assert.ok(existsSync(join(env.SEATLINE_DATA_DIR, "broker.sock")), "the broker did not start");

  // 1. The broker is reachable and the provider is signed in.
  const status = await ask({ request_id: "status", method: "provider.status", payload: { provider_id: "codex" } });
  const state = status.find(frame => frame.event === "provider.status")?.payload?.status;
  assert.equal(state?.authentication, "authenticated", `provider status: ${JSON.stringify(state)}`);

  // 2. With the grant, a plain question completes with the provider's answer.
  const plain = await ask(plainAsk("plain_allowed"));
  assert.equal(terminal(plain).event, "response.completed", JSON.stringify(terminal(plain)));
  assert.ok(plain.some(frame => frame.event === "response.delta" && String(frame.payload?.text).includes("Brokered answer")));

  // 3. A question that carries page context completes as well.
  const context = await ask(contextAsk("context_allowed"));
  assert.equal(terminal(context).event, "response.completed", JSON.stringify(terminal(context)));

  // 4. Without the grant the plain question is refused, and says how to fix it.
  authorize(ORIGIN);
  const refused = terminal(await ask(plainAsk("plain_denied")));
  assert.equal(refused.event, "response.failed");
  assert.equal(refused.payload.error.code, "INVALID_REQUEST");
  assert.equal(refused.payload.error.reason, "PROVIDER_DEFAULT_TOOLS_DENIED");
  assert.match(refused.payload.error.message, /--allow-provider-default/);
  assert.equal(refused.payload.error.retryable, false);

  // 5. ...but context asks never needed it.
  const stillContext = await ask(contextAsk("context_denied"));
  assert.equal(terminal(stillContext).event, "response.completed", JSON.stringify(terminal(stillContext)));
  console.log("Brokered asks passed: plain and context asks complete with the grant; the plain ask names the missing grant without it");
} finally {
  broker.kill();
  await once(broker, "exit").catch(() => {});
  rmSync(root, { recursive: true, force: true });
}
