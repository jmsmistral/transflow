"""Reject drift from the interpreter-specific contributor tooling lock."""

import importlib.metadata
import re
import sys
from pathlib import Path


def main() -> int:
    """Check the environment without installing or resolving dependencies."""
    minor = sys.version_info[:2]
    if minor != (3, 14):
        print("Python checks require Python 3.14", file=sys.stderr)
        return 1
    lock = Path(__file__).resolve().parents[1] / f"dev-py{minor[0]}{minor[1]}.lock"
    pins = dict(re.findall(r"^([A-Za-z0-9_-]+)==([^\s\\]+)", lock.read_text(), re.M))
    if not pins:
        print(f"No dependency pins found in {lock.name}", file=sys.stderr)
        return 1
    for name, expected in pins.items():
        try:
            actual = importlib.metadata.version(name)
        except importlib.metadata.PackageNotFoundError:
            actual = "missing"
        if actual != expected:
            print(
                f"Python tooling drift: {name} is {actual}, expected {expected}. "
                f"Install python/{lock.name} with pip --require-hashes.",
                file=sys.stderr,
            )
            return 1
    print(f"Python {sys.version.split()[0]}: all {len(pins)} tooling pins match {lock.name}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
