import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import Ajv2020 from "ajv/dist/2020.js";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, "..");
const SCHEMA_DIR = path.join(ROOT, "docs", "protocol", "schemas");
const FIXTURE_DIR = path.join(ROOT, "docs", "protocol", "fixtures");

function loadJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function compileSchema(ajv, name) {
  return ajv.compile(loadJson(path.join(SCHEMA_DIR, name)));
}

function formatErrors(validate) {
  return (validate.errors ?? [])
    .map((error) => `- ${error.instancePath || "/"}: ${error.message}`)
    .join("\n");
}

function expectValid(value, validate, label) {
  if (!validate(value)) {
    throw new Error(`${label} unexpectedly failed schema validation:\n${formatErrors(validate)}`);
  }
}

function expectInvalid(value, validate, label) {
  if (validate(value)) {
    throw new Error(`${label} unexpectedly passed schema validation`);
  }
}

const ajv = new Ajv2020({ allErrors: true, strict: false });
const requestValidator = compileSchema(ajv, "request-envelope.schema.json");
const eventValidator = compileSchema(ajv, "event-envelope.schema.json");
const errorValidator = compileSchema(ajv, "error.schema.json");
const providerStatusValidator = compileSchema(ajv, "provider-status.schema.json");

const golden = loadJson(path.join(FIXTURE_DIR, "v1-golden.json"));
const errorsCapabilities = loadJson(path.join(FIXTURE_DIR, "v1-errors-capabilities.json"));

for (const fixture of golden.valid_requests) {
  expectValid(fixture.value, requestValidator, `request fixture: ${fixture.name}`);
}

for (const fixture of golden.valid_events) {
  const value = fixture.value;
  expectValid(value, eventValidator, `event fixture: ${fixture.name}`);

  if (value.event === "response.failed") {
    expectValid(value.payload.error, errorValidator, `error payload: ${fixture.name}`);
  }

  if (value.event === "provider.status") {
    expectValid(value.payload, providerStatusValidator, `provider status payload: ${fixture.name}`);
  }
}

for (const fixture of golden.invalid_cases) {
  if (Object.hasOwn(fixture, "value")) {
    expectInvalid(fixture.value, requestValidator, `invalid request fixture: ${fixture.name}`);
    continue;
  }

  let parsed = false;
  try {
    JSON.parse(fixture.raw);
    parsed = true;
  } catch {
    // Expected malformed JSON.
  }
  if (parsed) {
    throw new Error(`malformed JSON fixture unexpectedly parsed: ${fixture.name}`);
  }
}

for (const [index, error] of errorsCapabilities.valid_errors.entries()) {
  expectValid(error, errorValidator, `normalized error fixture #${index + 1}`);
}

for (const [index, status] of errorsCapabilities.provider_statuses.entries()) {
  expectValid(status, providerStatusValidator, `provider status fixture #${index + 1}`);
}

const schemaCodes = new Set(errorValidator.schema.properties.code.enum);
const fixtureCodes = new Set(errorsCapabilities.error_codes);
if (
  schemaCodes.size !== fixtureCodes.size ||
  [...schemaCodes].some((code) => !fixtureCodes.has(code))
) {
  throw new Error(
    `error-code fixture/schema drift: fixture=${JSON.stringify([...fixtureCodes].sort())} ` +
      `schema=${JSON.stringify([...schemaCodes].sort())}`,
  );
}

const requiredCapabilities = new Set(
  providerStatusValidator.schema.properties.status.properties.capabilities.required,
);
const fixtureCapabilities = new Set(errorsCapabilities.capability_keys);
if (
  requiredCapabilities.size !== fixtureCapabilities.size ||
  [...requiredCapabilities].some((key) => !fixtureCapabilities.has(key))
) {
  throw new Error(
    `capability fixture/schema drift: fixture=${JSON.stringify([...fixtureCapabilities].sort())} ` +
      `schema=${JSON.stringify([...requiredCapabilities].sort())}`,
  );
}

console.log(
  "Protocol validation passed: " +
    `${golden.valid_requests.length} requests, ` +
    `${golden.valid_events.length} events, ` +
    `${golden.invalid_cases.length} invalid cases, ` +
    `${errorsCapabilities.valid_errors.length} errors, ` +
    `${errorsCapabilities.provider_statuses.length} provider statuses.`,
);
