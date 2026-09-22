import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const dist = path.join(root, "dist");
const source = path.join(root, "src");
const manifest = path.join(root, "manifest.json");

const parsedManifest = JSON.parse(fs.readFileSync(manifest, "utf8"));

if (parsedManifest.manifest_version !== 3) {
  throw new Error("extension build requires a Manifest V3 manifest");
}

fs.rmSync(dist, { recursive: true, force: true });
fs.mkdirSync(dist, { recursive: true });
fs.copyFileSync(manifest, path.join(dist, "manifest.json"));
fs.cpSync(source, path.join(dist, "src"), { recursive: true });

const required = [
  parsedManifest.action.default_popup,
  parsedManifest.background.service_worker,
  ...parsedManifest.content_scripts.flatMap((entry) => entry.js)
];

for (const relativePath of required) {
  if (!fs.existsSync(path.join(dist, relativePath))) {
    throw new Error(`built extension is missing ${relativePath}`);
  }
}

console.log(`Built extension to ${path.relative(process.cwd(), dist)}`);
