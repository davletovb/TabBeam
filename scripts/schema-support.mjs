import path from "node:path";
import Ajv2020 from "ajv/dist/2020.js";
import { ROOT, loadJson } from "./protocol-support.mjs";

export const SCHEMA_DIR = path.join(ROOT, "docs", "protocol", "schemas");

export function createAjv() {
  return new Ajv2020({ allErrors: true });
}

export function compileSchema(ajv, name) {
  return ajv.compile(loadJson(path.join(SCHEMA_DIR, name)));
}

export function formatSchemaErrors(validate, prefix = "") {
  return (validate.errors ?? [])
    .map((error) => `${prefix}${error.instancePath || "/"}: ${error.message}`)
    .join("\n");
}
