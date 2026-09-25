#!/usr/bin/env python3

import argparse
import json
import os
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

from jsonschema import Draft202012Validator

ROOT = Path(__file__).resolve().parents[1]
SCHEMA_DIR = ROOT / "docs" / "protocol" / "schemas"
FIXTURE_PATH = ROOT / "docs" / "protocol" / "fixtures" / "v1-golden.json"

# Chrome launches the host with the caller's origin, and the host refuses to
# run without one (SEC-01). Any well-formed extension ID works here.
CALLER_ORIGIN = "chrome-extension://" + "a" * 32 + "/"


def load_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def frame(payload: bytes) -> bytes:
    return struct.pack("=I", len(payload)) + payload


def parse_frames(data: bytes):
    frames = []
    offset = 0
    while offset < len(data):
        if len(data) - offset < 4:
            raise AssertionError("host output ended with a partial frame prefix")
        (length,) = struct.unpack_from("=I", data, offset)
        offset += 4
        if len(data) - offset < length:
            raise AssertionError("host output ended with a partial frame payload")
        raw = data[offset : offset + length]
        offset += length
        frames.append(json.loads(raw.decode("utf-8")))
    return frames


def validate_event(event, event_validator, error_validator, status_validator):
    errors = list(event_validator.iter_errors(event))
    if errors:
        raise AssertionError(
            "host event failed event-envelope schema: "
            + "; ".join(error.message for error in errors)
        )

    if event["event"] == "response.failed":
        errors = list(error_validator.iter_errors(event["payload"]["error"]))
        if errors:
            raise AssertionError(
                "host error payload failed schema: "
                + "; ".join(error.message for error in errors)
            )

    if event["event"] == "provider.status":
        errors = list(status_validator.iter_errors(event["payload"]))
        if errors:
            raise AssertionError(
                "host provider-status payload failed schema: "
                + "; ".join(error.message for error in errors)
            )


def validate_diagnostics(stderr: bytes, request_count: int) -> int:
    """Checks the host's stderr (OBS-01): one JSON record per line, from
    host.started through one record per request to a clean host.stopped."""
    records = [json.loads(line) for line in stderr.decode("utf-8").splitlines()]
    events = [record.get("event") for record in records]
    if events[:1] != ["host.started"] or events[-1:] != ["host.stopped"]:
        raise AssertionError(f"diagnostics do not start and stop the host: {events}")
    request_events = events[1:-1]
    if len(request_events) != request_count or not all(
        event.startswith("request.") for event in request_events
    ):
        raise AssertionError(
            f"expected {request_count} request records, got {request_events}"
        )
    if records[-1].get("exit_code") != 0:
        raise AssertionError(f"host.stopped reports {records[-1]}")
    return len(records)


def collapse_repeats(events: list, repeatable: set) -> list:
    collapsed = []
    for event in events:
        if collapsed and event == collapsed[-1] and event in repeatable:
            continue
        collapsed.append(event)
    return collapsed


def compact(value) -> bytes:
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", required=True, type=Path)
    args = parser.parse_args()

    golden = load_json(FIXTURE_PATH)
    event_validator = Draft202012Validator(load_json(SCHEMA_DIR / "event-envelope.schema.json"))
    error_validator = Draft202012Validator(load_json(SCHEMA_DIR / "error.schema.json"))
    status_validator = Draft202012Validator(load_json(SCHEMA_DIR / "provider-status.schema.json"))

    driven = []
    input_bytes = bytearray()

    for fixture in golden["valid_requests"]:
        if fixture.get("host_conformance") == "deferred":
            continue
        input_bytes += frame(compact(fixture["value"]))
        driven.append(("valid", fixture))

    for fixture in golden["invalid_cases"]:
        if "value" in fixture:
            payload = compact(fixture["value"])
        else:
            payload = fixture["raw"].encode("utf-8")
        input_bytes += frame(payload)
        driven.append(("invalid", fixture))

    # An empty provider search path keeps the run independent of whatever
    # provider CLIs this machine has installed.
    with tempfile.TemporaryDirectory() as no_providers:
        proc = subprocess.run(
            [str(args.host), CALLER_ORIGIN],
            input=bytes(input_bytes),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            env={**os.environ, "PERVUE_PROVIDER_PATH": no_providers},
        )
    if proc.returncode != 0:
        raise AssertionError(
            f"host exited {proc.returncode}: {proc.stderr.decode('utf-8', errors='replace')}"
        )

    diagnostics = validate_diagnostics(proc.stderr, len(driven))

    events = parse_frames(proc.stdout)
    if not events or events[0].get("event") != "host.ready":
        raise AssertionError("first host frame is not host.ready")

    for event in events:
        validate_event(event, event_validator, error_validator, status_validator)

    remaining = events[1:]
    by_request = {}
    null_events = []
    for event in remaining:
        request_id = event["request_id"]
        if request_id is None:
            null_events.append(event)
        else:
            by_request.setdefault(request_id, []).append(event)

    sequence_map = {item["name"]: item for item in golden["sequences"]}

    null_index = 0
    for kind, fixture in driven:
        if kind == "valid":
            request_id = fixture["value"]["request_id"]
            sequence_name = fixture.get("sequence")
            if not sequence_name:
                raise AssertionError(f"missing sequence metadata for {fixture['name']}")
            actual = [event["event"] for event in by_request.get(request_id, [])]
            sequence = sequence_map[sequence_name]
            expected = sequence["events"]
            # A repeatable event may occur several times in a row, such as
            # one provider.status per provider (v1 §7.5).
            actual = collapse_repeats(actual, set(sequence.get("repeatable", [])))
            if actual != expected:
                raise AssertionError(
                    f"{fixture['name']} sequence mismatch: expected {expected}, got {actual}"
                )
            continue

        expected = fixture["expected"]
        request_id = expected["request_id"]
        if request_id is None:
            if null_index >= len(null_events):
                raise AssertionError(f"missing null-correlated event for {fixture['name']}")
            candidates = [null_events[null_index]]
            null_index += 1
        else:
            candidates = by_request.get(request_id, [])

        if len(candidates) != 1:
            raise AssertionError(
                f"{fixture['name']} expected one event, got {len(candidates)}"
            )

        event = candidates[0]
        if event["event"] != expected["event"]:
            raise AssertionError(
                f"{fixture['name']} event mismatch: {event['event']}"
            )
        error = event["payload"]["error"]
        if error["code"] != expected["error_code"] or error["reason"] != expected["reason"]:
            raise AssertionError(
                f"{fixture['name']} error mismatch: {error['code']}/{error['reason']}"
            )
        if error["retryable"] is not False:
            raise AssertionError(f"{fixture['name']} must be non-retryable")

    print(
        f"Host conformance passed: {len(events)} emitted frames, "
        f"{sum(1 for kind, _ in driven if kind == 'valid')} valid requests, "
        f"{sum(1 for kind, _ in driven if kind == 'invalid')} invalid requests, "
        f"{diagnostics} diagnostics records."
    )


if __name__ == "__main__":
    main()
