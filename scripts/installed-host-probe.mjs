import { spawn } from "node:child_process";
import { once } from "node:events";
import { frameNativeMessage, parseNativeFrames } from "./protocol-support.mjs";

const TERMINAL_EVENTS = new Set(["response.completed", "response.failed"]);

export async function probeInstalledHost(
  host,
  args,
  {
    providerId = "codex",
    requestId = "package_status",
    timeoutMs = 25_000,
    maxOutputBytes = 1024 * 1024,
    windowsHide = false
  } = {}
) {
  const child = spawn(host, args, {
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide
  });

  const exit = Promise.race([
    once(child, "close"),
    once(child, "error").then(([error]) => {
      throw error;
    })
  ]);

  // If the host exits before consuming the request, keep EPIPE/EOF from
  // masking the host's own stderr and exit status.
  child.stdin.on("error", () => {});

  let stderr = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", chunk => { stderr += chunk; });

  const timer = setTimeout(() => child.kill(), timeoutMs);
  try {
    const request = Buffer.from(JSON.stringify({
      version: 1,
      type: "request",
      request_id: requestId,
      method: "provider.status",
      payload: { provider_id: providerId }
    }));
    child.stdin.write(frameNativeMessage(request));

    let output = Buffer.alloc(0);
    let events = [];
    for await (const chunk of child.stdout) {
      output = Buffer.concat([output, chunk]);
      if (output.length >= maxOutputBytes) {
        throw new Error("host output exceeded verification limit");
      }
      try {
        events = parseNativeFrames(output);
      } catch (error) {
        if (!/partial frame (prefix|payload)/.test(String(error))) throw error;
        continue;
      }
      if (events.some(event =>
        event.request_id === requestId && TERMINAL_EVENTS.has(event.event)
      )) {
        break;
      }
    }

    child.stdin.end();
    const [code, signal] = await exit;
    return { code, signal, stderr, events };
  } finally {
    clearTimeout(timer);
    child.kill();
  }
}
