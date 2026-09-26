import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, "..", "..");
const PREFIX_SIZE = 4;

const loadJson = (filePath) => JSON.parse(fs.readFileSync(filePath, "utf8"));
const maxFrameBytes = () => loadJson(path.join(ROOT, "docs", "protocol", "native-messaging-v1.json")).max_frame_bytes;

function prefix(length) {
  const buffer = Buffer.alloc(PREFIX_SIZE);
  if (os.endianness() === "LE") buffer.writeUInt32LE(length);
  else buffer.writeUInt32BE(length);
  return buffer;
}

function createFrameCorpus(output) {
  fs.mkdirSync(output, { recursive: true });
  const maxFrame = maxFrameBytes();
  const seeds = new Map([
    ["empty-frame.bin", prefix(0)],
    ["small-binary.bin", Buffer.concat([prefix(6), Buffer.from([0, 1, 10, 26, 127, 255])])],
    ["max-frame.bin", Buffer.concat([prefix(maxFrame), Buffer.alloc(maxFrame, 0xa5)])],
    ["oversized-prefix.bin", prefix(maxFrame + 1)],
    ["truncated-prefix.bin", prefix(5).subarray(0, 2)],
    ["truncated-payload.bin", Buffer.concat([prefix(5), Buffer.from("abc")])],
  ]);
  for (const [name, data] of seeds) fs.writeFileSync(path.join(output, name), data);
  console.log(`Wrote ${seeds.size} Native Messaging fuzz seeds using ${os.endianness().toLowerCase()}-endian native prefixes.`);
}

function createProtocolCorpus(output) {
  fs.mkdirSync(output, { recursive: true });
  const golden = loadJson(path.join(ROOT, "docs", "protocol", "fixtures", "v1-golden.json"));
  let index = 0;
  const safeName = (name) => name.replaceAll(" ", "-");
  for (const fixture of golden.valid_requests) {
    fs.writeFileSync(
      path.join(output, `${String(index++).padStart(2, "0")}-valid-${safeName(fixture.name)}.json`),
      Buffer.from(JSON.stringify(fixture.value), "utf8"),
    );
  }
  for (const fixture of golden.invalid_cases) {
    const data = Object.hasOwn(fixture, "value")
      ? Buffer.from(JSON.stringify(fixture.value), "utf8")
      : Buffer.from(fixture.raw, "utf8");
    fs.writeFileSync(
      path.join(output, `${String(index++).padStart(2, "0")}-invalid-${safeName(fixture.name)}.bin`),
      data,
    );
  }

  const extras = new Map([
    ["duplicate-version.json", Buffer.from('{"version":1,"version":1,"type":"request","request_id":"req_dup","method":"provider.status","payload":{}}')],
    ["empty-object-trailing.bin", Buffer.from("{}x")],
    ["zero-byte.bin", Buffer.alloc(0)],
    ["escaped-method.json", Buffer.from('{"version":1,"type":"request","request_id":"req_escape","method":"conversation.sen\\u0064","payload":{"provider_id":"fak\\u0065","input":{"text":"Hello"}}}')],
  ]);
  for (const [name, data] of extras) fs.writeFileSync(path.join(output, name), data);
  console.log(`Wrote ${index + extras.size} protocol fuzz seeds.`);
}

function usage() {
  console.error("usage: fuzz-support.mjs frame-corpus <output> | protocol-corpus <output> | max-len <frame_reader|protocol>");
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
