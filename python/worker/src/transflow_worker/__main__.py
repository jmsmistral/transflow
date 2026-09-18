"""Support isolated invocation via ``python -I -m transflow_worker``."""

from .cli import main

if __name__ == "__main__":
    raise SystemExit(main())
