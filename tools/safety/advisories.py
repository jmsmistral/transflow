"""Explicit bounded OSV queries. Only public package names and versions are sent."""

import argparse
import datetime as dt
import json
import urllib.request
from pathlib import Path

from baseline import Failure, canonical, check_inventory, digest, require

PROVIDER = "https://api.osv.dev/v1/querybatch"


def post(queries):
    request = urllib.request.Request(
        PROVIDER,
        data=json.dumps({"queries": queries}).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=45) as response:
        raw = response.read(8 * 1024 * 1024 + 1)
    require(len(raw) <= 8 * 1024 * 1024, "OSV response exceeds size limit")
    return json.loads(raw)


def query(dependencies, request=post):
    packages = dependencies["packages"]
    findings = set()
    for start in range(0, len(packages), 200):
        batch = packages[start : start + 200]
        pending = [
            (
                p,
                {
                    "package": {"ecosystem": p["ecosystem"], "name": p["name"]},
                    "version": p["version"],
                },
                set(),
            )
            for p in batch
        ]
        for _page in range(20):
            response = request([q for _, q, _ in pending])
            require(
                isinstance(response, dict)
                and isinstance(response.get("results"), list)
                and len(response["results"]) == len(pending),
                "Incomplete OSV batch response",
            )
            next_pending = []
            for (package, params, seen), result in zip(pending, response["results"], strict=True):
                require(
                    isinstance(result, dict) and set(result) <= {"vulns", "next_page_token"},
                    "Malformed OSV result",
                )
                vulns = result.get("vulns", [])
                require(isinstance(vulns, list), "Malformed OSV vulnerabilities")
                for vuln in vulns:
                    require(
                        isinstance(vuln, dict) and isinstance(vuln.get("id"), str) and vuln["id"],
                        "Missing OSV advisory ID",
                    )
                    findings.add((package["id"], vuln["id"]))
                token = result.get("next_page_token")
                if token:
                    require(
                        isinstance(token, str) and token not in seen,
                        "Repeated/invalid OSV page token",
                    )
                    next_pending.append((package, {**params, "page_token": token}, seen | {token}))
            pending = next_pending
            if not pending:
                break
        require(not pending, "OSV pagination limit reached; audit is incomplete")
    return {
        "schema": 1,
        "provider": PROVIDER,
        "checked_at": dt.datetime.now(dt.UTC).isoformat(),
        "inventory_sha256": digest(canonical(dependencies)),
        "queried": [p["id"] for p in packages],
        "findings": [{"package": p, "advisory": a} for p, a in sorted(findings)],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    dependencies = check_inventory(root)
    try:
        report = query(dependencies)
    except (OSError, ValueError, KeyError) as error:
        raise Failure("OSV audit failed or is incomplete; no report was written") from error
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(canonical(report))
    print(
        f"Queried {len(report['queried'])} locked package versions; "
        f"found {len(report['findings'])} advisory matches. Apply the policy with check.py."
    )


if __name__ == "__main__":
    main()
