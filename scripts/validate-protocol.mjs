import path from "node:path";
import { loadJson, sameSet } from "./protocol-support.mjs";
import {
  SCHEMA_DIR,
  compileSchema,
  createAjv,
  formatSchemaErrors,
} from "./schema-support.mjs";

const FIXTURE_DIR = path.join(path.dirname(SCHEMA_DIR), "fixtures");

function expectValid(value, validate, label) {
  if (!validate(value)) {
    throw new Error(
      `${label} unexpectedly failed schema validation:\n${formatSchemaErrors(validate, "- ")}`,
    );
  }
}

function expectInvalid(value, validate, label) {
  if (validate(value)) {
    throw new Error(`${label} unexpectedly passed schema validation`);
  }
}

const ajv = createAjv();
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

  if (typeof fixture.raw !== "string") {
    throw new Error(`invalid fixture must contain either value or raw: ${fixture.name}`);
  }

  try {
    JSON.parse(fixture.raw);
  } catch (error) {
    if (error instanceof SyntaxError) continue;
    throw error;
  }
  throw new Error(`malformed JSON fixture unexpectedly parsed: ${fixture.name}`);
}

for (const [index, error] of errorsCapabilities.valid_errors.entries()) {
  expectValid(error, errorValidator, `normalized error fixture #${index + 1}`);
}

for (const [index, status] of errorsCapabilities.provider_statuses.entries()) {
  expectValid(status, providerStatusValidator, `provider status fixture #${index + 1}`);
}

const schemaCodes = new Set(errorValidator.schema.properties.code.enum);
const fixtureCodes = new Set(errorsCapabilities.error_codes);
if (!sameSet(schemaCodes, fixtureCodes)) {
  throw new Error(
    `error-code fixture/schema drift: fixture=${JSON.stringify([...fixtureCodes].sort())} ` +
      `schema=${JSON.stringify([...schemaCodes].sort())}`,
  );
}

const requiredCapabilities = new Set(
  providerStatusValidator.schema.properties.status.properties.capabilities.required,
);
const fixtureCapabilities = new Set(errorsCapabilities.capability_keys);
if (!sameSet(requiredCapabilities, fixtureCapabilities)) {
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
