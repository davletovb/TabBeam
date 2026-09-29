#!/usr/bin/env node
// The provider runtime must not depend on Pervue (ADR-0002): a crate whose name
// does not start with `pervue` may not depend, in any dependency section, on one
// that does. The runtime crates are meant to move to a repository of their own,
// and a dependency on the application would make that move impossible.
//
// The rule is read from the crates' names, so a new runtime crate is held to it
// without being listed. The crates listed below only guard against the check
// passing for want of something to check.

import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const MANIFESTS = [
  "native/Cargo.toml",
  "native/fuzz/Cargo.toml",
  "native/runtime-fuzz/Cargo.toml",
];
const APPLICATION = /^pervue(-|$)/;
const EXPECTED_RUNTIME_CRATES = [
  "runtime-core",
  "runtime-platform",
  "runtime-providers",
  "provider-runtime-scheduler",
  "provider-runtime-service",
  "runtime-fake-provider",
  "runtime-tests",
  "runtime-fuzz",
];

function packagesOf(manifest) {
  let output;
  try {
    output = execFileSync(
      "cargo",
      [
        "metadata",
        "--format-version",
        "1",
        "--no-deps",
        "--locked",
        "--manifest-path",
        path.join(ROOT, manifest),
      ],
      { encoding: "utf8", maxBuffer: 64 * 1024 * 1024, stdio: ["ignore", "pipe", "pipe"] },
    );
  } catch (error) {
    // Fail closed, and say why: a dependency added without refreshing the lock
    // file (which `--locked` refuses) ends up here too.
    const reason = String(error.stderr || error.message).trim().split("\n").slice(0, 5).join("\n");
    console.error(
      `Could not read ${manifest}, so its crates were not checked:\n${reason}\n` +
        "If a dependency was just added, refresh that manifest's Cargo.lock and run this again.",
    );
    process.exit(1);
  }
  return JSON.parse(output).packages;
}

const packages = MANIFESTS.flatMap(packagesOf);
const runtime = packages.filter((pkg) => !APPLICATION.test(pkg.name));
const violations = [];
for (const pkg of runtime) {
  for (const dependency of pkg.dependencies) {
    if (APPLICATION.test(dependency.name)) {
      violations.push(
        `${pkg.name} depends on ${dependency.name} (${dependency.kind ?? "normal"})`,
      );
    }
  }
}

const found = new Set(runtime.map((pkg) => pkg.name));
const missing = EXPECTED_RUNTIME_CRATES.filter((name) => !found.has(name));
if (missing.length) {
  console.error(
    `Expected runtime crates not found: ${missing.join(", ")}. ` +
      "If they were renamed or removed, update scripts/check-runtime-independence.mjs.",
  );
  process.exit(1);
}
if (violations.length) {
  console.error("The provider runtime must not depend on Pervue:");
  for (const violation of violations) console.error(`  ${violation}`);
  process.exit(1);
}
console.log(
  `Runtime independence OK: ${runtime.length} crates, none depends on a pervue* crate.`,
);
