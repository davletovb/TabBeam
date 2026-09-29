#!/usr/bin/env node
import fs from "node:fs";

const expectedRepo = "https://github.com/davletovb/seatline";
const manifest = fs.readFileSync(new URL("../native/Cargo.toml", import.meta.url), "utf8");
const lines = manifest.split(/\r?\n/).filter((line) => /^seatline-[a-z-]+\s*=/.test(line.trim()));
if (lines.length !== 5) {
  console.error(`Expected 5 Seatline workspace dependency pins, found ${lines.length}`);
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
if (manifest.includes('path = "../seatline-core"') || manifest.includes('path = "../platform"')) {
  console.error("An in-tree Seatline path dependency remains");
  process.exit(1);
}
if (process.env.GITHUB_OUTPUT) {
  fs.appendFileSync(process.env.GITHUB_OUTPUT, `rev=${rev}\n`);
}
console.log(`Seatline pin OK: ${expectedRepo}@${rev}`);
