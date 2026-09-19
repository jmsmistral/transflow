"""Synthetic failures: no real credentials or private reference bytes."""

import copy
import datetime as dt
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from advisories import query
from baseline import (
    LOCKS,
    Failure,
    apply_exceptions,
    check_advisories,
    check_inventory,
    digest,
    inventory,
    scan_bytes,
    scan_repository,
)
from contracts import check_contracts

ROOT = Path(__file__).resolve().parents[3]
TODAY = dt.date(2026, 9, 19)


class StandaloneCommandTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="transflow-standalone-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name) / "transflow"
        self.root.mkdir()
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        # A complete minimal checkout, deliberately without a sibling specification.
        paths = [*LOCKS, *[str(p.relative_to(ROOT)) for p in (ROOT / "security").glob("*.json")]]
        paths.extend(str(p.relative_to(ROOT)) for p in (ROOT / "tools/safety").glob("*.py"))
        for name in paths:
            destination = self.root / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, destination)
        # Synthetic fresh provider evidence; this fixture never queries the network.
        report_path = self.root / "security/advisories.json"
        report = json.loads(report_path.read_text())
        report["checked_at"] = dt.datetime.now(dt.UTC).isoformat()
        report["findings"] = []
        report_path.write_text(json.dumps(report))
        policy_path = self.root / "security/policy.json"
        policy = json.loads(policy_path.read_text())
        policy["exceptions"] = [
            item for item in policy["exceptions"] if item["finding"].startswith("deprecated:")
        ]
        policy_path.write_text(json.dumps(policy))

    def check(self, *arguments):
        return subprocess.run(
            [sys.executable, "-B", "tools/safety/check.py", *arguments],
            cwd=self.root,
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )

    def test_implementation_only_passes_without_spec_checkout(self):
        result = self.check("--implementation-only")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Scope: implementation only", result.stdout)
        self.assertIn("Safety baseline passed", result.stdout)

    def test_default_local_check_still_requires_spec_checkout(self):
        result = self.check()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SCREENSHOTS.md", result.stderr)

    def test_implementation_only_still_rejects_credentials(self):
        secret = "ghp_" + "B" * 36
        (self.root / "example.py").write_text('token = "' + secret + '"')
        result = self.check("--implementation-only")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("github-token", result.stdout)
        self.assertNotIn(secret, result.stdout + result.stderr)


class RepositoryTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="transflow-safety-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)

    def write(self, name, data):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(data)

    def test_seeded_secret_in_example_is_detected_and_redacted(self):
        secret = "ghp_" + "A" * 36
        self.write("examples/demo.py", '\nTOKEN = "' + secret + '"\n')
        count, findings = scan_repository(self.root, set())
        self.assertEqual(count, 1)
        self.assertEqual(findings[0]["rule"], "github-token")
        self.assertEqual(findings[0]["line"], 2)
        self.assertNotIn(secret, json.dumps(findings))

    def test_all_supported_secret_patterns(self):
        examples = [
            ("aws-access-key", "AKIA" + "Z" * 16),
            ("private-key", "-----BEGIN " + "PRIVATE KEY-----"),
            ("assigned-secret", 'api_key = "' + "xY9" * 10 + '"'),
        ]
        for rule, data in examples:
            with self.subTest(rule=rule):
                self.assertIn(rule, [f["rule"] for f in scan_bytes(data.encode(), "sample")])

    def test_plain_documentation_is_not_a_secret(self):
        self.assertEqual(
            scan_bytes(b'password = "example"\napi_key = os.environ["KEY"]', "demo"), []
        )

    def test_ignored_private_file_is_never_opened(self):
        self.write(".gitignore", "references/screenshots/\n")
        self.write("references/screenshots/private.png", "sensitive reference")
        original = Path.open

        def guarded(path, *args, **kwargs):
            self.assertNotEqual(path.name, "private.png")
            return original(path, *args, **kwargs)

        with patch.object(Path, "open", guarded):
            count, findings = scan_repository(self.root, {"private.png"})
        self.assertEqual((count, findings), (1, []))

    def test_tracked_ignored_private_path_fails_even_if_deleted(self):
        self.write(".gitignore", "*.png\n")
        self.write("private.png", "reference")
        subprocess.run(["git", "-C", str(self.root), "add", "-f", "private.png"], check=True)
        (self.root / "private.png").unlink()
        with self.assertRaisesRegex(Failure, "Private path"):
            scan_repository(self.root, {"private.png"})

    def test_symlink_is_rejected(self):
        (self.root / "outside").symlink_to(self.root.parent / "not-read")
        with self.assertRaisesRegex(Failure, "Symlink"):
            scan_repository(self.root, set())

    def test_file_size_limit_fails_closed(self):
        self.write("large", "too many bytes")
        with (
            patch("baseline.MAX_FILE_BYTES", 4),
            self.assertRaisesRegex(Failure, "limit"),
        ):
            scan_repository(self.root, set())

    def test_non_root_scan_is_rejected(self):
        (self.root / "nested").mkdir()
        with self.assertRaisesRegex(Failure, "explicit Git"):
            scan_repository(self.root / "nested", set())

    def test_generated_contract_success_and_drift(self):
        generator = (
            'from pathlib import Path\nPath("generated").mkdir()\n'
            'Path("generated/contract.json").write_text(Path("input.txt").read_text())\n'
        )
        self.write("generator.py", generator)
        self.write("input.txt", '{"version": 1}\n')
        self.write("generated/contract.json", '{"version": 1}\n')
        registry = {
            "schema": 1,
            "contracts": [
                {
                    "name": "synthetic",
                    "inputs": {
                        name: digest((self.root / name).read_bytes())
                        for name in ("generator.py", "input.txt")
                    },
                    "outputs": ["generated/contract.json"],
                    "command": ["{python}", "generator.py"],
                }
            ],
        }
        self.write("security/generated-contracts.json", json.dumps(registry))
        self.assertEqual(check_contracts(self.root), 1)
        self.write("generated/contract.json", "stale")
        with self.assertRaisesRegex(Failure, "drift"):
            check_contracts(self.root)
        self.assertEqual((self.root / "generated/contract.json").read_text(), "stale")

    def test_changed_contract_input_requires_review(self):
        self.write("generator.py", "")
        self.write(
            "security/generated-contracts.json",
            json.dumps(
                {
                    "schema": 1,
                    "contracts": [
                        {
                            "name": "synthetic",
                            "inputs": {"generator.py": "0" * 64},
                            "outputs": ["generated/a"],
                            "command": ["{python}", "generator.py"],
                        }
                    ],
                }
            ),
        )
        with self.assertRaisesRegex(Failure, "input changed"):
            check_contracts(self.root)

    def test_unregistered_contract_fails(self):
        self.write("security/generated-contracts.json", '{"schema": 1, "contracts": []}')
        self.write("web/src/generated/api.ts", "type Data = string;")
        with self.assertRaisesRegex(Failure, "Unregistered"):
            check_contracts(self.root)


class AuditTests(unittest.TestCase):
    def setUp(self):
        self.dependencies = {
            "schema": 1,
            "locks": {},
            "packages": [
                {
                    "id": "PyPI:sample@1.0",
                    "ecosystem": "PyPI",
                    "name": "sample",
                    "version": "1.0",
                }
            ],
        }

    def report(self):
        report = query(self.dependencies, lambda _: {"results": [{}]})
        report["checked_at"] = "2026-09-19T00:00:00+00:00"
        return report

    def test_clean_complete_report(self):
        self.assertEqual(check_advisories(self.report(), self.dependencies, TODAY), set())

    def test_pagination_preserves_all_findings(self):
        requests = []

        def request(queries):
            requests.append(queries)
            if len(requests) == 1:
                return {"results": [{"vulns": [{"id": "TEST-1"}], "next_page_token": "next"}]}
            return {"results": [{"vulns": [{"id": "TEST-2"}]}]}

        report = query(self.dependencies, request)
        self.assertEqual(len(report["findings"]), 2)
        self.assertEqual(requests[1][0]["page_token"], "next")
        self.assertEqual(set(requests[0][0]), {"package", "version"})

    def test_incomplete_or_error_response_never_passes(self):
        for response in (
            {},
            {"results": []},
            {"results": [{"error": "unavailable"}]},
            {"results": [{"vulns": None}]},
        ):
            with self.subTest(response=response), self.assertRaises(Failure):
                query(self.dependencies, lambda _, response=response: response)

    def test_network_failure_propagates(self):
        def unavailable(_):
            raise OSError("offline")

        with self.assertRaises(OSError):
            query(self.dependencies, unavailable)

    def test_repeating_page_token_fails(self):
        with self.assertRaisesRegex(Failure, "Repeated"):
            query(self.dependencies, lambda _: {"results": [{"next_page_token": "loop"}]})

    def test_unbounded_pagination_fails(self):
        counter = 0

        def more(_):
            nonlocal counter
            counter += 1
            return {"results": [{"next_page_token": str(counter)}]}

        with self.assertRaisesRegex(Failure, "pagination limit"):
            query(self.dependencies, more)

    def test_stale_future_incomplete_and_mismatched_reports_fail(self):
        mutations = (
            {"checked_at": "2026-09-10T00:00:00Z"},
            {"checked_at": "2026-09-20T00:00:00Z"},
            {"queried": []},
            {"inventory_sha256": "invalid"},
        )
        for changes in mutations:
            with self.subTest(changes=changes), self.assertRaises(Failure):
                check_advisories({**self.report(), **changes}, self.dependencies, TODAY)

    def policy(self):
        return {
            "schema": 1,
            "exceptions": [
                {
                    "finding": "advisory:TEST-1:PyPI:sample@1.0",
                    "owner": "Test maintainers",
                    "reason": "Synthetic exception for an exact package version.",
                    "expires": "2026-10-01",
                }
            ],
        }

    def test_exact_exception_and_unexcepted_finding(self):
        key = self.policy()["exceptions"][0]["finding"]
        self.assertEqual(apply_exceptions({key}, self.policy(), TODAY), [])
        self.assertEqual(
            apply_exceptions({key, "advisory:TEST-2:PyPI:sample@1.0"}, self.policy(), TODAY),
            ["advisory:TEST-2:PyPI:sample@1.0"],
        )

    def test_expired_unused_duplicate_and_wildcard_exceptions_fail(self):
        key = self.policy()["exceptions"][0]["finding"]
        cases = []
        for field, value in [
            ("expires", "2026-09-19"),
            ("owner", ""),
            ("reason", ""),
            ("finding", "advisory:*"),
        ]:
            p = self.policy()
            p["exceptions"][0][field] = value
            cases.append(p)
        duplicate = self.policy()
        duplicate["exceptions"].append(copy.deepcopy(duplicate["exceptions"][0]))
        cases.append(duplicate)
        for policy in cases:
            with self.subTest(policy=policy), self.assertRaises(Failure):
                apply_exceptions({key}, policy, TODAY)
        with self.assertRaisesRegex(Failure, "unused"):
            apply_exceptions(set(), self.policy(), TODAY)

    def test_current_lock_inventory_covers_all_sources(self):
        data = inventory(ROOT)
        self.assertEqual({p["ecosystem"] for p in data["packages"]}, {"crates.io", "PyPI", "npm"})
        self.assertEqual(len(data["locks"]), 9)

    def test_new_lockfile_requires_inventory_review(self):
        from baseline import git_files

        with (
            patch(
                "baseline.git_files",
                return_value=git_files(ROOT) + ["new/requirements.lock"],
            ),
            self.assertRaisesRegex(Failure, "coverage changed"),
        ):
            inventory(ROOT)

    def test_inventory_drift_and_missing_license_fail(self):
        from baseline import read_json

        original = read_json(ROOT / "security/dependencies.json")
        for mutate in ("lock", "license", "version"):
            saved = copy.deepcopy(original)
            if mutate == "lock":
                saved["locks"]["Cargo.lock"] = "bad"
            elif mutate == "license":
                saved["packages"][0]["licenses"] = []
            else:
                saved["packages"][0]["version"] = "bad"
            with (
                patch("baseline.read_json", return_value=saved),
                self.assertRaises(Failure),
            ):
                check_inventory(ROOT)


if __name__ == "__main__":
    unittest.main()
