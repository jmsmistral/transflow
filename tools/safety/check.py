"""Run the offline T008 safety baseline. Refresh network data separately."""

import argparse
import datetime as dt
import sys
from pathlib import Path

from baseline import (
    Failure,
    apply_exceptions,
    canonical,
    check_advisories,
    check_inventory,
    private_names,
    read_json,
    require,
    scan_repository,
)
from contracts import check_contracts


def main():
    root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    scope = parser.add_mutually_exclusive_group()
    scope.add_argument(
        "--spec-root", type=Path, help="Sibling specification checkout for local checks"
    )
    scope.add_argument(
        "--implementation-only",
        action="store_true",
        help="Check only this repository without reading private specification files",
    )
    parser.add_argument("--advisories", type=Path, default=root / "security/advisories.json")
    args = parser.parse_args()
    if args.implementation_only:
        names = set()
        repositories = (root,)
        print("Scope: implementation only; spec and mapped screenshot names are not checked.")
    else:
        spec_root = args.spec_root or root.parent / "transflow-spec"
        names = private_names(spec_root)
        repositories = (root, spec_root)
    total = 0
    for repo in repositories:
        count, secrets = scan_repository(repo, names)
        total += count
        if secrets:
            print(canonical({"repository": repo.name, "findings": secrets}).decode())
        require(
            not secrets,
            f"Possible credentials detected in {repo.name}; diagnostics above are redacted",
        )
    dependencies = check_inventory(root)
    today = dt.datetime.now(dt.UTC).date()
    findings = check_advisories(read_json(args.advisories), dependencies, today)
    findings.update(f"deprecated:{p['id']}" for p in dependencies["packages"] if p["deprecations"])
    policy = read_json(root / "security/policy.json")
    remaining = apply_exceptions(findings, policy, today)
    require(not remaining, f"Unexcepted dependency findings: {remaining}")
    contracts = check_contracts(root)
    print(
        f"Safety baseline passed: {total} public candidate files, "
        f"{len(dependencies['packages'])} locked package versions, "
        f"{len(policy['exceptions'])} active exceptions, "
        f"{contracts} registered generated contracts."
    )
    if not contracts:
        print(
            "No application contracts are generated yet; "
            "T012/T014/T074 must register their generators."
        )


if __name__ == "__main__":
    try:
        main()
    except (Failure, OSError, ValueError, KeyError, TypeError) as error:
        print(f"Safety check failed: {error}", file=sys.stderr)
        sys.exit(1)
