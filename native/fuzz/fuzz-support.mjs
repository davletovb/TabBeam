import fs from "node:fs";
import path from "node:path";
import {
  jsonValueWireBytes,
  loadJson,
  nativeLengthPrefix,
  NATIVE_ENDIAN_NAME,
  ROOT,
} from "../../scripts/protocol-support.mjs";

const PREFIX_SIZE = 4;
const CONTRACT = path.join(ROOT, "docs", "protocol", "native-messaging-v1.json");
const GOLDEN = path.join(ROOT, "docs", "protocol", "fixtures", "v1-golden.json");

function maxFrameBytes() {
  const value = loadJson(CONTRACT).max_frame_bytes;
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new Error("max_frame_bytes must be a positive safe integer");
  }
  return value;
}

function createFrameCorpus(output) {
  fs.mkdirSync(output, { recursive: true });
  const maxFrame = maxFrameBytes();
  const seeds = new Map([
    ["empty-frame.bin", nativeLengthPrefix(0)],
    [
      "small-binary.bin",
      Buffer.concat([nativeLengthPrefix(6), Buffer.from([0, 1, 10, 26, 127, 255])]),
    ],
    [
      "max-frame.bin",
      Buffer.concat([nativeLengthPrefix(maxFrame), Buffer.alloc(maxFrame, 0xa5)]),
    ],
    ["oversized-prefix.bin", nativeLengthPrefix(maxFrame + 1)],
    ["truncated-prefix.bin", nativeLengthPrefix(5).subarray(0, 2)],
    ["truncated-payload.bin", Buffer.concat([nativeLengthPrefix(5), Buffer.from("abc")])],
  ]);

  for (const [name, data] of seeds) fs.writeFileSync(path.join(output, name), data);
  console.log(
    `Wrote ${seeds.size} Native Messaging fuzz seeds using ${NATIVE_ENDIAN_NAME}-endian native prefixes.`,
  );
}

function createProtocolCorpus(output) {
  fs.mkdirSync(output, { recursive: true });
  const golden = loadJson(GOLDEN);
  let index = 0;
  const safeName = (name) => name.replaceAll(" ", "-");

  for (const [fixtureIndex, fixture] of golden.valid_requests.entries()) {
    fs.writeFileSync(
      path.join(output, `${String(index++).padStart(2, "0")}-valid-${safeName(fixture.name)}.json`),
      jsonValueWireBytes(GOLDEN, ["valid_requests", fixtureIndex, "value"]),
    );
  }

  for (const [fixtureIndex, fixture] of golden.invalid_cases.entries()) {
    let data;
    if (Object.hasOwn(fixture, "value")) {
      data = jsonValueWireBytes(GOLDEN, ["invalid_cases", fixtureIndex, "value"]);
    } else if (typeof fixture.raw === "string") {
      data = Buffer.from(fixture.raw, "utf8");
    } else {
      throw new Error(`invalid fixture must contain either value or raw: ${fixture.name}`);
    }
    fs.writeFileSync(
      path.join(output, `${String(index++).padStart(2, "0")}-invalid-${safeName(fixture.name)}.bin`),
      data,
    );
  }

  const extras = new Map([
    [
      "duplicate-version.json",
      Buffer.from(
        '{"version":1,"version":1,"type":"request","request_id":"req_dup","method":"provider.status","payload":{}}',
      ),
    ],
    ["empty-object-trailing.bin", Buffer.from("{}x")],
    ["zero-byte.bin", Buffer.alloc(0)],
    [
      "escaped-method.json",
      Buffer.from(
        '{"version":1,"type":"request","request_id":"req_escape","method":"conversation.sen\\u0064","payload":{"provider_id":"fak\\u0065","input":{"text":"Hello"}}}',
      ),
    ],
  ]);

  for (const [name, data] of extras) fs.writeFileSync(path.join(output, name), data);
  console.log(`Wrote ${index + extras.size} protocol fuzz seeds.`);
}

function usage() {
  console.error(
    "usage: fuzz-support.mjs frame-corpus <output> | protocol-corpus <output> | " +
      "max-len <frame_reader|protocol>",
  );
  process.exit(64);
}

const [command, argument, ...extra] = process.argv.slice(2);
if (!command || !argument || extra.length) usage();

switch (command) {
  case "frame-corpus":
    createFrameCorpus(path.resolve(argument));
    break;
  case "protocol-corpus":
    createProtocolCorpus(path.resolve(argument));
    break;
  case "max-len":
    if (argument === "frame_reader") console.log(maxFrameBytes() + PREFIX_SIZE);
    else if (argument === "protocol") console.log(maxFrameBytes());
    else usage();
    break;
  default:
    usage();
}
