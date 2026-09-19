"""T011 bounded resource, POSIX cancellation and offline resolver feasibility tests."""

from __future__ import annotations

import argparse
import importlib.metadata
import json
import platform
import signal
import sys
import tempfile
import unittest
from pathlib import Path

from managed import Managed
from policy import Limits, UnsupportedLimit
from resolver import qualify

OBSERVATIONS = {}


class ResourceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="transflow-resource-case-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()

    def test_default_policies_and_optional_reservations(self):
        defaults = Limits()
        self.assertEqual(
            (defaults.compute, defaults.validation, defaults.interactive), (3600, 3600, 30)
        )
        self.assertIsNone(defaults.memory_budget)
        self.assertIsNone(defaults.hard_memory)
        self.assertTrue(defaults.admits(2**80, 2**80))
        optional = Limits(memory_budget=100)
        self.assertTrue(optional.admits(60, 40))
        self.assertFalse(optional.admits(61, 40))
        OBSERVATIONS["memory_policy"] = {
            "default_admission_gate": False,
            "optional_reservation_enforced": True,
            "reservation_is_not_RSS_limit": True,
        }

    def test_invalid_and_unsupported_limits(self):
        for phase in ("compute", "validation", "interactive"):
            for value in (-1, float("nan"), float("inf"), True):
                with self.subTest(phase=phase, value=value), self.assertRaises(ValueError):
                    Limits(**{phase: value})
        for field in ("memory_budget", "hard_memory"):
            for value in (0, -1, True, 1.5):
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    Limits(**{field: value})
        with self.assertRaisesRegex(UnsupportedLimit, "not supported.*any platform"):
            Limits(hard_memory=1024)
        for values in ((-1, 0), (1, -1), (True, 0)):
            with self.assertRaises(ValueError):
                Limits().admits(*values)
        OBSERVATIONS["hard_process_limits"] = "unsupported; rejected explicitly on every platform"

    def test_phase_budgets_do_not_reset_per_query_or_share_job_deadline(self):
        limits = Limits(compute=100, validation=80, interactive=5)
        compute = limits.start("compute", 10)
        self.assertFalse(compute.expired(109))
        self.assertTrue(compute.expired(110))
        validation = limits.start("validation", 109)
        self.assertFalse(validation.expired(150))
        # All queries in this validation phase share the original start, including query 2.
        self.assertTrue(validation.expired(189))
        output_validation = limits.start("validation", 300)
        self.assertFalse(output_validation.expired(301))
        self.assertTrue(output_validation.expired(380))
        with self.assertRaisesRegex(ValueError, "Unknown phase"):
            limits.start("typo", 0)

    def test_validation_outlives_interactive_timer(self):
        limits = Limits(validation=3600, interactive=30)
        with (
            Managed(self.root, "duckdb", limits.start("validation", 0)) as validation,
            Managed(self.root, "leaf", limits.start("interactive", 0)) as interactive,
        ):
            self.assertIsNone(validation.tick(31))
            self.assertIsNone(validation.process.poll())
            report = interactive.tick(31)
            self.assertEqual(report["outcome"], "ERROR")
            self.assertEqual(report["phase"], "interactive")
            self.assertIsNotNone(interactive.process.returncode)
            self.assertIsNone(validation.tick(3599))
            self.assertIsNone(validation.process.poll())
            timeout = validation.tick(3600)
            self.assertEqual(timeout["outcome"], "ERROR")
            self.assertEqual(timeout["phase"], "validation")
            self.assertEqual(timeout["setting_source"], "workspace")
            self.assertIn("Increase", timeout["message"])
            self.assertIn("logs", timeout)
            OBSERVATIONS["independent_timers"] = {
                "clock": "injected elapsed seconds; real children",
                "interactive_expired_at": 31,
                "validation_alive_at": 3599,
                "validation_error": timeout,
            }

    def test_disabled_deadlines_remain_cancellable_for_both_engines(self):
        results = {}
        for phase, engine in (("compute", "polars"), ("validation", "duckdb")):
            with (
                self.subTest(engine=engine),
                Managed(
                    self.root, engine, Limits(compute=0, validation=0).start(phase, 0)
                ) as child,
            ):
                self.assertIsNone(child.tick(10**9))
                self.assertIsNone(child.process.poll())
                self.assertEqual(child.ready["threads"], 1)
                self.assertGreater(child.ready["max_rss_bytes"], 0)
                result = child.cancel()
                self.assertEqual(result, {"outcome": "CANCELED", "phase": phase})
                self.assertEqual(child.process.returncode, -signal.SIGTERM)
                results[engine] = {**child.ready, "cancelled": True, "reaped": True}
        OBSERVATIONS["engine_settings_and_cancellation"] = results

    def test_graceful_group_cancellation_reaps_descendant(self):
        with Managed(self.root, "tree", Limits().start("compute", 0)) as child:
            child.cancel()
            descendant = json.loads(child.stdout)
            self.assertTrue(descendant["descendant_reaped"])
            self.assertEqual(descendant["returncode"], -signal.SIGTERM)
            self.assertEqual(child.process.returncode, 0)
            self.assertFalse(child.escalated)
            OBSERVATIONS["process_tree"] = {"descendant_reaped": True, "parent_reaped": True}

    def test_uncooperative_child_is_killed_after_grace(self):
        with Managed(self.root, "stubborn", Limits(compute=0).start("compute", 0)) as child:
            self.assertEqual(child.stop(grace=0.05), -signal.SIGKILL)
            self.assertTrue(child.escalated)
            OBSERVATIONS["escalation"] = {"grace_seconds": 0.05, "SIGKILL": True, "reaped": True}

    def test_exception_cleanup_preserves_candidate_and_last_good(self):
        head = self.root / "last-good"
        candidate = self.root / "candidate"
        head.write_bytes(b"last good immutable bytes")
        candidate.write_bytes(b"unpublished candidate bytes")
        with self.assertRaisesRegex(RuntimeError, "injected caller failure"):
            with Managed(self.root, "leaf", Limits().start("compute", 0)) as child:
                raise RuntimeError("injected caller failure")
        self.assertIsNotNone(child.process.returncode)
        self.assertEqual(head.read_bytes(), b"last good immutable bytes")
        self.assertEqual(candidate.read_bytes(), b"unpublished candidate bytes")

    def test_opt_in_duckdb_buffer_limit_is_not_a_process_limit(self):
        import duckdb

        with duckdb.connect(config={"threads": 1, "enable_external_access": False}) as db:
            default = db.execute("SELECT current_setting('memory_limit')").fetchone()[0]
            db.execute("SET memory_limit = '1MB'")
            db.execute("SET max_temp_directory_size = '0B'")
            effective = db.execute("SELECT current_setting('memory_limit')").fetchone()[0]
            with self.assertRaises(duckdb.OutOfMemoryException):
                db.execute("SELECT i FROM range(100000) t(i) ORDER BY hash(i)").fetchall()
            self.assertEqual(db.execute("SELECT 1").fetchone(), (1,))
            OBSERVATIONS["duckdb_opt_in_limit"] = {
                "engine_default": default,
                "requested": "1MB",
                "effective": effective,
                "spill": "disabled for test",
                "over_budget_query": "OutOfMemoryException",
                "scope": "engine buffer manager; not total process RSS",
            }

    def test_pinned_resolver_offline_hashes_and_install(self):
        OBSERVATIONS["resolver"] = qualify(self.root)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    expected = {"pip": "26.2.1", "pip-tools": "7.6.1", "polars": "1.44.2", "duckdb": "1.5.5"}
    versions = {name: importlib.metadata.version(name) for name in expected}
    if versions != expected or sys.version_info[:2] not in {(3, 13), (3, 14)}:
        raise RuntimeError("Use the qualified Python and locked resolver/engine environment")
    result = unittest.TextTestRunner(verbosity=2).run(
        unittest.defaultTestLoader.loadTestsFromTestCase(ResourceTests)
    )
    report = {
        "task": "T011",
        "python": platform.python_version(),
        "platform": platform.system(),
        "machine": platform.machine(),
        "packages": versions,
        "tests": result.testsRun,
        "failures": len(result.failures),
        "errors": len(result.errors),
        "observations": OBSERVATIONS,
        "limits": [
            "Feasibility only; not production resource admission or worker supervision",
            "Phase time uses an injected clock; child cancellation uses real POSIX signals",
            "No cgroup backend or universal RSS ceiling",
            "Only fixed bounded-output fixture processes; no hostile-process containment",
        ],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    sys.exit(main())
