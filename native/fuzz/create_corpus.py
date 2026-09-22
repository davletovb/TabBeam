#!/usr/bin/env python3

import argparse
import struct
import sys
from pathlib import Path

MAX_FRAME = 1024 * 1024


def prefix(length: int) -> bytes:
    return struct.pack("=I", length)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    args.output.mkdir(parents=True, exist_ok=True)

    seeds = {
        "empty-frame.bin": prefix(0),
        "small-binary.bin": prefix(6) + bytes([0, 1, 10, 26, 127, 255]),
        "max-frame.bin": prefix(MAX_FRAME) + bytes([0xA5]) * MAX_FRAME,
        "oversized-prefix.bin": prefix(MAX_FRAME + 1),
        "truncated-prefix.bin": prefix(5)[:2],
        "truncated-payload.bin": prefix(5) + b"abc",
    }

    for name, data in seeds.items():
        (args.output / name).write_bytes(data)

    print(
        f"Wrote {len(seeds)} Native Messaging fuzz seeds "
        f"using {sys.byteorder}-endian native prefixes."
    )


if __name__ == "__main__":
    main()
