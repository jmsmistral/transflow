"""Inject dependency defects into metadata obtained from the actual workspace."""

import copy
import json
from pathlib import Path
import subprocess
import sys
import tomllib
import unittest

TOOLS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(TOOLS))
import check_rust


class RustBoundaries(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.root = TOOLS.parent
        cls.baseline = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--locked", "--offline", "--no-deps"], cwd=cls.root, text=True
        ))
        data = tomllib.loads((cls.root / "tools/qualification/rust/Cargo.toml").read_text())["dependencies"]
        cls.pins = {name: value if isinstance(value, str) else value["version"] for name, value in data.items()}

    def setUp(self):
        self.metadata = copy.deepcopy(self.baseline)
        self.packages = {package["name"]: package for package in self.metadata["packages"]}

    def add_internal(self, source, target):
        self.packages[source]["dependencies"].append({"name": target, "source": None, "path": str(self.root / "crates" / target), "req": "*"})

    def messages(self):
        return "\n".join(check_rust.validate_graph(self.metadata, self.pins))

    def test_real_workspace_and_locks(self):
        self.assertEqual(check_rust.validate_checkout(self.root), [])

    def test_allowed_dependency_direction(self):
        self.add_internal("tf-protocol", "tf-domain")
        self.add_internal("tf-catalog", "tf-protocol")
        self.add_internal("transflow", "tf-catalog")
        self.assertEqual(self.messages(), "")

    def test_domain_cannot_depend_on_storage(self):
        self.add_internal("tf-domain", "tf-store")
        self.assertIn("Forbidden crate dependency: tf-domain -> tf-store", self.messages())

    def test_cycle_reports_path(self):
        self.add_internal("tf-domain", "tf-protocol")
        self.add_internal("tf-protocol", "tf-domain")
        self.assertIn("Crate dependency cycle: tf-domain -> tf-protocol -> tf-domain", self.messages())

    def test_cycle_omits_acyclic_prefix_and_duplicate_edges(self):
        self.add_internal("tf-catalog", "tf-domain")
        self.add_internal("tf-domain", "tf-protocol")
        self.add_internal("tf-protocol", "tf-domain")
        self.add_internal("tf-protocol", "tf-domain")
        cycles = [m for m in self.messages().splitlines() if m.startswith("Crate dependency cycle:")]
        self.assertEqual(cycles, ["Crate dependency cycle: tf-domain -> tf-protocol -> tf-domain"])

    def test_alias_optional_target_and_build_dependencies_do_not_bypass_policy(self):
        self.packages["tf-domain"]["dependencies"].append({
            "name": "sqlx", "rename": "innocent_name", "optional": True,
            "target": 'cfg(target_os = "linux")', "kind": "build",
            "source": "registry+https://github.com/rust-lang/crates.io-index", "req": "=0.9.0",
        })
        self.assertIn("Unapproved external dependency: tf-domain -> sqlx", self.messages())

    def test_local_crate_cannot_be_substituted_from_registry(self):
        self.add_internal("tf-protocol", "tf-domain")
        dependency = next(d for d in self.packages["tf-protocol"]["dependencies"] if d["name"] == "tf-domain")
        dependency["source"] = "registry+https://github.com/rust-lang/crates.io-index"
        self.assertIn("must use the local workspace crate", self.messages())

    def test_exact_external_pins(self):
        self.packages["transflow"]["dependencies"][0]["req"] = "^4.6.7"
        self.assertIn("version differs from the exact qualification pin", self.messages())

    def test_foreign_path_is_not_a_qualified_dependency(self):
        dependency = self.packages["transflow"]["dependencies"][0]
        dependency["source"] = None
        dependency["path"] = "/not-a-transflow-root"
        self.assertIn("unqualified dependency source", self.messages())

    def test_missing_crate(self):
        self.metadata["workspace_members"].remove(self.packages["tf-store"]["id"])
        self.assertIn("exactly the ten specified crates", self.messages())

    def test_test_executor_permission_does_not_allow_runtime_or_build_dependency(self):
        dependency = next(d for d in self.packages["tf-store"]["dependencies"] if d["name"] == "tokio")
        self.assertEqual(dependency["kind"], "dev")
        self.assertEqual(self.messages(), "")
        for kind in [None, "build"]:
            dependency["kind"] = kind
            self.assertIn("Unapproved external dependency: tf-store -> tokio", self.messages())

    def test_toolchain_and_publication_drift(self):
        self.packages["tf-domain"].update(edition="2021", rust_version="1.80", publish=None)
        self.assertIn("edition/MSRV drift", self.messages())
        self.assertIn("publish=false", self.messages())


if __name__ == "__main__":
    unittest.main()
