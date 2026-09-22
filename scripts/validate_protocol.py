#!/usr/bin/env python3

import json
from pathlib import Path

from jsonschema import Draft202012Validator
from jsonschema.exceptions import ValidationError

ROOT = Path(__file__).resolve().parents[1]
SCHEMA_DIR = ROOT / "docs" / "protocol" / "schemas"
FIXTURE_DIR = ROOT / "docs" / "protocol" / "fixtures"


def load_json(path: Path):
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def validator(name: str) -> Draft202012Validator:
    schema = load_json(SCHEMA_DIR / name)
    Draft202012Validator.check_schema(schema)
    return Draft202012Validator(schema)


def expect_valid(instance, schema_validator, label: str):
    errors = sorted(schema_validator.iter_errors(instance), key=lambda error: list(error.path))
    if errors:
        details = "\n".join(f"- {error.json_path}: {error.message}" for error in errors)
        raise AssertionError(f"{label} unexpectedly failed schema validation:\n{details}")


def expect_invalid(instance, schema_validator, label: str):
    try:
        schema_validator.validate(instance)
    except ValidationError:
        return
    raise AssertionError(f"{label} unexpectedly passed schema validation")


def main():
    request_validator = validator("request-envelope.schema.json")
    event_validator = validator("event-envelope.schema.json")
    error_validator = validator("error.schema.json")
    provider_status_validator = validator("provider-status.schema.json")

    golden = load_json(FIXTURE_DIR / "v1-golden.json")
    errors_capabilities = load_json(FIXTURE_DIR / "v1-errors-capabilities.json")

    for fixture in golden["valid_requests"]:
        expect_valid(fixture["value"], request_validator, f"request fixture: {fixture['name']}")

    for fixture in golden["valid_events"]:
        value = fixture["value"]
        expect_valid(value, event_validator, f"event fixture: {fixture['name']}")

        if value["event"] == "response.failed":
            expect_valid(
                value["payload"]["error"],
                error_validator,
                f"error payload: {fixture['name']}",
            )

        if value["event"] == "provider.status":
            expect_valid(
                value["payload"],
                provider_status_validator,
                f"provider status payload: {fixture['name']}",
            )

    for fixture in golden["invalid_cases"]:
        if "value" in fixture:
            expect_invalid(
                fixture["value"],
                request_validator,
                f"invalid request fixture: {fixture['name']}",
            )
        else:
            try:
                json.loads(fixture["raw"])
            except json.JSONDecodeError:
                pass
            else:
                raise AssertionError(
                    f"malformed JSON fixture unexpectedly parsed: {fixture['name']}"
                )

    for index, error in enumerate(errors_capabilities["valid_errors"]):
        expect_valid(error, error_validator, f"normalized error fixture #{index + 1}")

    for index, status in enumerate(errors_capabilities["provider_statuses"]):
        expect_valid(status, provider_status_validator, f"provider status fixture #{index + 1}")

    schema_codes = set(error_validator.schema["properties"]["code"]["enum"])
    fixture_codes = set(errors_capabilities["error_codes"])
    if fixture_codes != schema_codes:
        raise AssertionError(
            f"error-code fixture/schema drift: fixture={sorted(fixture_codes)} schema={sorted(schema_codes)}"
        )

    required_capabilities = set(
        provider_status_validator.schema["properties"]["status"]["properties"]["capabilities"]["required"]
    )
    fixture_capabilities = set(errors_capabilities["capability_keys"])
    if fixture_capabilities != required_capabilities:
        raise AssertionError(
            "capability fixture/schema drift: "
            f"fixture={sorted(fixture_capabilities)} schema={sorted(required_capabilities)}"
        )

    print(
        "Protocol validation passed: "
        f"{len(golden['valid_requests'])} requests, "
        f"{len(golden['valid_events'])} events, "
        f"{len(golden['invalid_cases'])} invalid cases, "
        f"{len(errors_capabilities['valid_errors'])} errors, "
        f"{len(errors_capabilities['provider_statuses'])} provider statuses."
    )


if __name__ == "__main__":
    main()
