"""Regenerate registered contracts in disposable directories and compare exact bytes."""

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from baseline import digest, git_files, read_json, require, safe_path


def check_contracts(root: Path) -> int:
    registry = read_json(root / "security/generated-contracts.json")
    require(
        set(registry) == {"schema", "contracts"} and registry["schema"] == 1,
        "Invalid generated-contract registry",
    )
    outputs, names = set(), set()
    for item in registry["contracts"]:
        require(
            set(item) == {"name", "inputs", "outputs", "command"},
            "Invalid generated-contract entry",
        )
        require(
            item["name"] not in names and item["inputs"] and item["outputs"] and item["command"],
            "Duplicate/empty contract",
        )
        names.add(item["name"])
        require(
            isinstance(item["command"], list)
            and all(isinstance(v, str) for v in item["command"])
            and item["command"][0] == "{python}",
            "Contract generator must use the prepared Python interpreter",
        )
        with tempfile.TemporaryDirectory(prefix="transflow-contract-") as temporary:
            destination = Path(temporary)
            for name, expected in item["inputs"].items():
                source = safe_path(root, name)
                require(
                    digest(source.read_bytes()) == expected,
                    f"Contract input changed: {name}; review and refresh the registry",
                )
                target = safe_path(destination, name)
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, target)
            # Generators are reviewed repository code, not a sandbox; no shell expansion.
            try:
                subprocess.run(
                    [sys.executable, *item["command"][1:]],
                    cwd=destination,
                    check=True,
                    timeout=60,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
            except (subprocess.SubprocessError, OSError) as error:
                raise ValueError(f"Contract generator failed: {item['name']}") from error
            for name in item["outputs"]:
                require(name not in outputs, f"Duplicate contract output: {name}")
                require(
                    safe_path(destination, name).read_bytes() == safe_path(root, name).read_bytes(),
                    f"Generated contract drift: {name}",
                )
                outputs.add(name)
    # Reserve these paths/names for generated artifacts as contracts arrive.
    detected = {
        p for p in git_files(root) if "/generated/" in f"/{p}" or ".generated." in Path(p).name
    }
    require(
        detected <= outputs,
        f"Unregistered generated artifacts: {sorted(detected - outputs)}",
    )
    return len(names)
