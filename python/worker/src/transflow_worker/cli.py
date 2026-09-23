"""Diagnostic worker entry point; stdout is not a control transport."""

import argparse
import json
import sys
from collections.abc import Sequence
from dataclasses import asdict
from pathlib import Path

from ._version import __version__
from .compatibility import CompatibilityError, check_compatibility


def main(argv: Sequence[str] | None = None) -> int:
    """Print help/version or verify the installed SDK/worker bootstrap pair."""
    parser = argparse.ArgumentParser(
        prog="transflow-worker",
        description="Private captured-source discovery and Polars execution worker.",
        add_help=False,
    )
    parser.add_argument("-h", "--help", action="store_true", help="show this help message")
    parser.add_argument("--version", action="store_true", help="show the package version")
    subcommands = parser.add_subparsers(dest="command")
    compatibility = subcommands.add_parser(
        "compatibility", help="check installed package compatibility", add_help=False
    )
    compatibility.add_argument("-h", "--help", action="store_true", dest="command_help")
    compatibility.add_argument("--protocol-major", type=int, default=1)
    compatibility.add_argument("--protocol-minor", type=int, default=0)
    discovery = subcommands.add_parser(
        "discover", help="import captured declarations on a private channel"
    )
    discovery.add_argument("--request", type=Path, required=True)
    discovery.add_argument("--control-socket", type=Path, required=True)
    execution = subcommands.add_parser("execute", help="materialize a pinned Polars producer")
    execution.add_argument("--request", type=Path, required=True)
    execution.add_argument("--control-socket", type=Path, required=True)
    evaluation = subcommands.add_parser(
        "evaluate_checks", help="evaluate canonical checks privately"
    )
    evaluation.add_argument("--request", type=Path, required=True)
    evaluation.add_argument("--control-socket", type=Path, required=True)
    try:
        args = parser.parse_args(argv)
        if args.help or args.command is None and not args.version:
            print(parser.format_help(), end="")
        elif args.version:
            print(f"transflow-worker {__version__}")
        elif args.command == "discover":
            from .discovery import DiscoveryError, serve
            from .wire import ProtocolError

            try:
                return serve(args.request, args.control_socket)
            except DiscoveryError, ProtocolError, OSError, ValueError:
                print(
                    "Discovery setup failed; verify the captured request "
                    "and private coordinator channel",
                    file=sys.stderr,
                )
                return 1
        elif args.command == "execute":
            from .discovery import DiscoveryError
            from .execution import serve as execute_serve
            from .wire import ProtocolError

            try:
                return execute_serve(args.request, args.control_socket)
            except DiscoveryError, ProtocolError, OSError, ValueError:
                print(
                    "Execution setup failed; verify the pinned request and private channel",
                    file=sys.stderr,
                )
                return 1
        elif args.command == "evaluate_checks":
            from .checks import serve as checks_serve
            from .discovery import DiscoveryError
            from .wire import ProtocolError

            try:
                return checks_serve(args.request, args.control_socket)
            except DiscoveryError, ProtocolError, OSError, ValueError:
                print(
                    "Check setup failed; verify the exact subject and private channel",
                    file=sys.stderr,
                )
                return 1
        elif args.command_help:
            print(compatibility.format_help(), end="")
        else:
            report = check_compatibility(
                protocol_major=args.protocol_major, protocol_minor=args.protocol_minor
            )
            print(json.dumps(asdict(report), sort_keys=True))
    except (CompatibilityError, OSError) as exc:
        context = (
            "Worker compatibility check failed"
            if isinstance(exc, CompatibilityError)
            else "Could not write worker diagnostic output"
        )
        try:
            print(f"{context}: {exc}", file=sys.stderr)
        except OSError:
            # Failure remains visible in the exit status when stderr is also closed.
            return 1
        return 1
    return 0
