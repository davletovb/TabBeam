import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { parseArgs } from "node:util";
import {
  decodeUtf8,
  frameNativeMessage,
  jsonValueWireBytes,
  loadJson,
  parseNativeFrames,
  ROOT,
} from "./protocol-support.mjs";
import { compileSchema, createAjv, formatSchemaErrors } from "./schema-support.mjs";

const FIXTURE_PATH = path.join(ROOT, "docs", "protocol", "fixtures", "v1-golden.json");
const CALLER_ORIGIN = `chrome-extension://${"a".repeat(32)}/`;

function validateEvent(event, eventValidator, errorValidator, statusValidator) {
  if (!eventValidator(event)) {
    throw new Error(`host event failed schema: ${formatSchemaErrors(eventValidator)}`);
  }
  if (event.event === "response.failed" && !errorValidator(event.payload.error)) {
    throw new Error(`host error payload failed schema: ${formatSchemaErrors(errorValidator)}`);
  }
  if (event.event === "provider.status" && !statusValidator(event.payload)) {
    throw new Error(
      `host provider-status payload failed schema: ${formatSchemaErrors(statusValidator)}`,
    );
  }
}

function validateDiagnostics(stderr, requestCount) {
  let text = decodeUtf8(stderr);
  if (text.endsWith("\n")) text = text.slice(0, -1);
  const lines = text.split("\n").map((line) => (line.endsWith("\r") ? line.slice(0, -1) : line));
  const records = lines.map((line) => JSON.parse(line));
  const events = records.map((record) => record.event);
  if (events[0] !== "host.started" || events.at(-1) !== "host.stopped") {
    throw new Error(`diagnostics do not start and stop the host: ${JSON.stringify(events)}`);
  }
  const requestEvents = events.slice(1, -1);
  if (
    requestEvents.length !== requestCount ||
    !requestEvents.every((event) => event?.startsWith("request."))
  ) {
    throw new Error(
      `expected ${requestCount} request records, got ${JSON.stringify(requestEvents)}`,
    );
  }
  if (records.at(-1).exit_code !== 0) {
    throw new Error(`host.stopped reports ${JSON.stringify(records.at(-1))}`);
  }
  return records.length;
}

function collapseRepeats(events, repeatable) {
  const collapsed = [];
  for (const event of events) {
    if (collapsed.at(-1) === event && repeatable.has(event)) continue;
    collapsed.push(event);
  }
  return collapsed;
}

function stderrForFailure(stderr) {
  try {
    return decodeUtf8(stderr);
  } catch (error) {
    return `<stderr is not valid UTF-8: ${error.message}>`;
  }
}

let values;
try {
  ({ values } = parseArgs({
    args: process.argv.slice(2),
    options: { host: { type: "string" } },
    allowPositionals: false,
    strict: true,
  }));
} catch (error) {
  console.error(`validate-host-protocol.mjs: ${error.message}`);
  process.exit(64);
}

if (!values.host) {
  console.error("usage: validate-host-protocol.mjs --host <path>");
  process.exit(64);
}
const invocationRoot = process.env.INIT_CWD ?? process.cwd();
const host = path.resolve(invocationRoot, values.host);

const golden = loadJson(FIXTURE_PATH);
const ajv = createAjv();
const eventValidator = compileSchema(ajv, "event-envelope.schema.json");
const errorValidator = compileSchema(ajv, "error.schema.json");
const statusValidator = compileSchema(ajv, "provider-status.schema.json");
const driven = [];
const input = [];

for (const [index, fixture] of golden.valid_requests.entries()) {
  if (fixture.host_conformance === "deferred") continue;
  input.push(
    frameNativeMessage(jsonValueWireBytes(FIXTURE_PATH, ["valid_requests", index, "value"])),
  );
  driven.push(["valid", fixture]);
}

for (const [index, fixture] of golden.invalid_cases.entries()) {
  let payload;
  if (Object.hasOwn(fixture, "value")) {
    payload = jsonValueWireBytes(FIXTURE_PATH, ["invalid_cases", index, "value"]);
  } else if (typeof fixture.raw === "string") {
    payload = Buffer.from(fixture.raw, "utf8");
  } else {
    throw new Error(`invalid fixture must contain either value or raw: ${fixture.name}`);
  }
  input.push(frameNativeMessage(payload));
  driven.push(["invalid", fixture]);
}

const noProviders = fs.mkdtempSync(path.join(os.tmpdir(), "tabbeam-no-providers-"));
let proc;
try {
  proc = spawnSync(host, [CALLER_ORIGIN], {
    input: Buffer.concat(input),
    env: { ...process.env, TABBEAM_PROVIDER_PATH: noProviders },
    encoding: null,
    maxBuffer: 64 * 1024 * 1024,
  });
} finally {
  fs.rmSync(noProviders, { recursive: true, force: true });
}

if (proc.error && ["ENOENT", "EACCES"].includes(proc.error.code)) {
  throw new Error(
    `failed to spawn host (${proc.error.code}): ${proc.error.message}`,
  );
}
if (proc.signal !== null) {
  throw new Error(
    `host exited by signal ${proc.signal}: ${stderrForFailure(proc.stderr ?? Buffer.alloc(0))}`,
  );
}
if (typeof proc.status === "number" && proc.status !== 0) {
  throw new Error(
    `host exited with status ${proc.status}: ${stderrForFailure(proc.stderr ?? Buffer.alloc(0))}`,
  );
}
if (proc.error) {
  throw new Error(
    `host I/O failed${proc.error.code ? ` (${proc.error.code})` : ""}: ${proc.error.message}; ` +
      `stderr: ${stderrForFailure(proc.stderr ?? Buffer.alloc(0))}`,
  );
}
if (proc.status !== 0) {
  throw new Error(`host ended without an exit status: ${stderrForFailure(proc.stderr ?? Buffer.alloc(0))}`);
}

const diagnostics = validateDiagnostics(proc.stderr, driven.length);
const events = parseNativeFrames(proc.stdout);
if (!events.length || events[0].event !== "host.ready") {
  throw new Error("first host frame is not host.ready");
}
for (const event of events) validateEvent(event, eventValidator, errorValidator, statusValidator);

const byRequest = new Map();
const nullEvents = [];
for (const event of events.slice(1)) {
  if (event.request_id === null) {
    nullEvents.push(event);
  } else {
    const requestEvents = byRequest.get(event.request_id) ?? [];
    requestEvents.push(event);
    byRequest.set(event.request_id, requestEvents);
  }
}

const sequenceMap = new Map(golden.sequences.map((sequence) => [sequence.name, sequence]));
let nullIndex = 0;
for (const [kind, fixture] of driven) {
  if (kind === "valid") {
    const sequence = sequenceMap.get(fixture.sequence);
    if (!sequence) throw new Error(`missing sequence metadata for ${fixture.name}`);
    const actual = collapseRepeats(
      (byRequest.get(fixture.value.request_id) ?? []).map((event) => event.event),
      new Set(sequence.repeatable ?? []),
    );
    if (JSON.stringify(actual) !== JSON.stringify(sequence.events)) {
      throw new Error(
        `${fixture.name} sequence mismatch: expected ${JSON.stringify(sequence.events)}, ` +
          `got ${JSON.stringify(actual)}`,
      );
    }
    continue;
  }

  const expected = fixture.expected;
  let candidates;
  if (expected.request_id === null) {
    if (nullIndex >= nullEvents.length) {
      throw new Error(`missing null-correlated event for ${fixture.name}`);
    }
    candidates = [nullEvents[nullIndex++]];
  } else {
    candidates = byRequest.get(expected.request_id) ?? [];
  }

  if (candidates.length !== 1) {
    throw new Error(`${fixture.name} expected one event, got ${candidates.length}`);
  }

  const event = candidates[0];
  if (event.event !== expected.event) {
    throw new Error(`${fixture.name} event mismatch: ${event.event}`);
  }
  const error = event.payload.error;
  if (error.code !== expected.error_code || error.reason !== expected.reason) {
    throw new Error(`${fixture.name} error mismatch: ${error.code}/${error.reason}`);
  }
  if (error.retryable !== false) throw new Error(`${fixture.name} must be non-retryable`);
}

console.log(
  `Host conformance passed: ${events.length} emitted frames, ` +
    `${driven.filter(([kind]) => kind === "valid").length} valid requests, ` +
    `${driven.filter(([kind]) => kind === "invalid").length} invalid requests, ` +
    `${diagnostics} diagnostics records.`,
);
