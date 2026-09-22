#!/usr/bin/env python3

import argparse
import json
from pathlib import Path


def compact(value) -> bytes:
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument(
        "--fixtures",
        type=Path,
        default=Path("docs/protocol/fixtures/v1-golden.json"),
    )
    args = parser.parse_args()

    args.output.mkdir(parents=True, exist_ok=True)
    golden = json.loads(args.fixtures.read_text(encoding="utf-8"))

    index = 0
    for fixture in golden["valid_requests"]:
        (args.output / f"{index:02d}-valid-{fixture['name'].replace(' ', '-')}.json").write_bytes(
            compact(fixture["value"])
        )
        index += 1

    for fixture in golden["invalid_cases"]:
        if "value" in fixture:
            data = compact(fixture["value"])
        else:
            data = fixture["raw"].encode("utf-8")
        (args.output / f"{index:02d}-invalid-{fixture['name'].replace(' ', '-')}.bin").write_bytes(data)
        index += 1

    extras = {
        "duplicate-version.json": (
            b'{"version":1,"version":1,"type":"request","request_id":"req_dup",'
            b'"method":"provider.status","payload":{}}'
        ),
        "empty-object-trailing.bin": b"{}x",
        "zero-byte.bin": b"",
        "escaped-method.json": (
            b'{"version":1,"type":"request","request_id":"req_escape",'
            b'"method":"conversation.sen\\u0064","payload":{"provider_id":"fak\\u0065",'
            b'"input":{"text":"Hello"}}}'
        ),
    }

    for name, data in extras.items():
        (args.output / name).write_bytes(data)

    print(f"Wrote {index + len(extras)} protocol fuzz seeds.")


if __name__ == "__main__":
    main()
