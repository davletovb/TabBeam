#!/usr/bin/env node
import fs from "node:fs";

const expectedRepo = "https://github.com/davletovb/seatline";
const manifest = fs.readFileSync(new URL("../native/Cargo.toml", import.meta.url), "utf8");
const lines = manifest.split(/\r?\n/).filter((line) => /^seatline-[a-z-]+\s*=/.test(line.trim()));
if (lines.length !== 6) {
  console.error(`Expected 6 Seatline workspace dependency pins, found ${lines.length}`);
  process.exit(1);
}
const revs = new Set();
for (const line of lines) {
  const git = line.match(/git\s*=\s*"([^"]+)"/)?.[1];
  const rev = line.match(/rev\s*=\s*"([0-9a-f]{40})"/)?.[1];
  if (git !== expectedRepo || !rev) {
    console.error(`Seatline dependency is not an exact repository+revision pin: ${line}`);
    process.exit(1);
  }
  revs.add(rev);
}
if (revs.size !== 1) {
  console.error(`Seatline crates are pinned to different revisions: ${[...revs].join(", ")}`);
  process.exit(1);
}
const rev=[...revs][0];
// The host only talks to the local broker, so it must not link the hosted
// transport (TLS, WebSocket and crypto) that Seatline keeps behind `web`.
const companion = lines.find((line) => /^seatline-companion\s*=/.test(line.trim()));
if (!companion || !/default-features\s*=\s*false/.test(companion)) {
  console.error("seatline-companion must be a client-only dependency: default-features = false");
  process.exit(1);
}
// CI installs the broker from Seatline too. It has to be the revision the host compiles against.
const workflows = new URL("../.github/workflows/", import.meta.url);
let installs = 0;
for (const name of fs.readdirSync(workflows).filter((file) => /\.ya?ml$/.test(file))) {
  const text = fs.readFileSync(new URL(name, workflows), "utf8");
  for (const match of text.matchAll(/github\.com\/davletovb\/seatline\s+--rev\s+([0-9a-f]{40})/g)) {
    installs++;
    if (match[1] !== rev) {
      console.error(`${name} installs the Seatline broker from ${match[1]}, but native/Cargo.toml pins ${rev}`);
      process.exit(1);
    }
  }
}
if (installs === 0) {
  console.error("No CI step installs the Seatline broker from the pinned revision");
  process.exit(1);
}
if (manifest.includes('path = "../seatline-core"') || manifest.includes('path = "../platform"')) {
  console.error("An in-tree Seatline path dependency remains");
  process.exit(1);
}
if (process.env.GITHUB_OUTPUT) {
  fs.appendFileSync(process.env.GITHUB_OUTPUT, `rev=${rev}\n`);
}
console.log(`Seatline pin OK: ${expectedRepo}@${rev} (${installs} CI install(s) match)`);
