import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import Ajv2020 from "ajv/dist/2020.js";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, "..");
const SCHEMA_DIR = path.join(ROOT, "docs", "protocol", "schemas");
const FIXTURE_PATH = path.join(ROOT, "docs", "protocol", "fixtures", "v1-golden.json");
const CALLER_ORIGIN = `chrome-extension://${"a".repeat(32)}/`;

const loadJson = (filePath) => JSON.parse(fs.readFileSync(filePath, "utf8"));
const compileSchema = (ajv, name) => ajv.compile(loadJson(path.join(SCHEMA_DIR, name)));
const errorsFor = (validate) =>
  (validate.errors ?? []).map((error) => `${error.instancePath || "/"}: ${error.message}`).join("; ");

function frame(payload) {
  const prefix = Buffer.alloc(4);
  if (os.endianness() === "LE") prefix.writeUInt32LE(payload.length);
  else prefix.writeUInt32BE(payload.length);
  return Buffer.concat([prefix, payload]);
}

function parseFrames(data) {
  const frames = [];
  let offset = 0;
  while (offset < data.length) {
    if (data.length - offset < 4) throw new Error("host output ended with a partial frame prefix");
    const length = os.endianness() === "LE" ? data.readUInt32LE(offset) : data.readUInt32BE(offset);
    offset += 4;
    if (data.length - offset < length) throw new Error("host output ended with a partial frame payload");
    frames.push(JSON.parse(data.subarray(offset, offset + length).toString("utf8")));
    offset += length;
  }
  return frames;
}

function validateEvent(event, eventValidator, errorValidator, statusValidator) {
  if (!eventValidator(event)) throw new Error(`host event failed schema: ${errorsFor(eventValidator)}`);
  if (event.event === "response.failed" && !errorValidator(event.payload.error)) {
    throw new Error(`host error payload failed schema: ${errorsFor(errorValidator)}`);
  }
  if (event.event === "provider.status" && !statusValidator(event.payload)) {
    throw new Error(`host provider-status payload failed schema: ${errorsFor(statusValidator)}`);
  }
}

function validateDiagnostics(stderr, requestCount) {
  const records = stderr.toString("utf8").split(/\r?\n/u).filter(Boolean).map((line) => JSON.parse(line));
  const events = records.map((record) => record.event);
  if (events[0] !== "host.started" || events.at(-1) !== "host.stopped") {
    throw new Error(`diagnostics do not start and stop the host: ${JSON.stringify(events)}`);
  }
  const requestEvents = events.slice(1, -1);
  if (requestEvents.length !== requestCount || !requestEvents.every((event) => event?.startsWith("request."))) {
    throw new Error(`expected ${requestCount} request records, got ${JSON.stringify(requestEvents)}`);
  }
  if (records.at(-1).exit_code !== 0) throw new Error(`host.stopped reports ${JSON.stringify(records.at(-1))}`);
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

const args = process.argv.slice(2);
if (args.length !== 2 || args[0] !== "--host") {
  console.error("usage: validate-host-protocol.mjs --host <path>");
  process.exit(64);
}
const host = path.resolve(args[1]);
const golden = loadJson(FIXTURE_PATH);
const ajv = new Ajv2020({ allErrors: true, strict: false });
const eventValidator = compileSchema(ajv, "event-envelope.schema.json");
const errorValidator = compileSchema(ajv, "error.schema.json");
const statusValidator = compileSchema(ajv, "provider-status.schema.json");
const driven = [];
const input = [];

for (const fixture of golden.valid_requests) {
  if (fixture.host_conformance === "deferred") continue;
  input.push(frame(Buffer.from(JSON.stringify(fixture.value), "utf8")));
  driven.push(["valid", fixture]);
}
for (const fixture of golden.invalid_cases) {
  const payload = Object.hasOwn(fixture, "value")
    ? Buffer.from(JSON.stringify(fixture.value), "utf8")
    : Buffer.from(fixture.raw, "utf8");
  input.push(frame(payload));
  driven.push(["invalid", fixture]);
}

const noProviders = fs.mkdtempSync(path.join(os.tmpdir(), "pervue-no-providers-"));
let proc;
try {
  proc = spawnSync(host, [CALLER_ORIGIN], {
    input: Buffer.concat(input),
    env: { ...process.env, PERVUE_PROVIDER_PATH: noProviders },
    encoding: null,
    maxBuffer: 64 * 1024 * 1024,
  });
} finally {
  fs.rmSync(noProviders, { recursive: true, force: true });
}

if (proc.error) throw proc.error;
if (proc.status !== 0) throw new Error(`host exited ${proc.status}: ${proc.stderr.toString("utf8")}`);

const diagnostics = validateDiagnostics(proc.stderr, driven.length);
const events = parseFrames(proc.stdout);
if (!events.length || events[0].event !== "host.ready") throw new Error("first host frame is not host.ready");
for (const event of events) validateEvent(event, eventValidator, errorValidator, statusValidator);

const byRequest = new Map();
const nullEvents = [];
for (const event of events.slice(1)) {
  if (event.request_id === null) nullEvents.push(event);
  else {
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
      throw new Error(`${fixture.name} sequence mismatch: expected ${JSON.stringify(sequence.events)}, got ${JSON.stringify(actual)}`);
    }
    continue;
  }

  const expected = fixture.expected;
  let candidates;
  if (expected.request_id === null) {
    if (nullIndex >= nullEvents.length) throw new Error(`missing null-correlated event for ${fixture.name}`);
    candidates = [nullEvents[nullIndex++]];
  } else {
    candidates = byRequest.get(expected.request_id) ?? [];
  }
  if (candidates.length !== 1) throw new Error(`${fixture.name} expected one event, got ${candidates.length}`);

  const event = candidates[0];
  if (event.event !== expected.event) throw new Error(`${fixture.name} event mismatch: ${event.event}`);
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
