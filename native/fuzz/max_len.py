#!/usr/bin/env python3
"""Prints libFuzzer's -max_len for a fuzz target, derived from the frame limit
the extension and the host share (docs/protocol/native-messaging-v1.json)."""

import argparse
import json
from pathlib import Path

CONTRACT = Path(__file__).resolve().parents[2] / "docs/protocol/native-messaging-v1.json"
PREFIX_SIZE = 4


def max_frame_bytes() -> int:
    return json.loads(CONTRACT.read_text(encoding="utf-8"))["max_frame_bytes"]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=["frame_reader", "protocol"])
    target = parser.parse_args().target
    # frame_reader inputs are a length prefix plus a payload; protocol inputs
    # are one payload.
    print(max_frame_bytes() + (PREFIX_SIZE if target == "frame_reader" else 0))


if __name__ == "__main__":
    main()
