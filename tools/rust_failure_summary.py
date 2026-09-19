"""Report only public-source test identifiers, never CI log excerpts or error text."""
from pathlib import Path
import re
import sys
import subprocess


def summary(log: str, known: set[str]) -> str:
    """Allowlist identifiers against Rust function names; discard every other byte."""
    names = sorted({
        match.group(1)
        for match in re.finditer(r"^test ([a-zA-Z_][a-zA-Z0-9_]*) \.\.\. FAILED$", log, re.M)
        if match.group(1) in known
    })[:20]
    return "Failed public Rust tests: " + (", ".join(names) or "no named test failure recorded") + "\n"


if __name__ == "__main__":
    root = Path(__file__).resolve().parent.parent
    known = set()
    tracked = subprocess.check_output(
        ["git", "ls-files", "-z", "--", "crates", "tests"], cwd=root
    ).decode().split("\0")
    for name in tracked:
        if name.endswith(".rs"):
            known.update(re.findall(r"\bfn ([a-zA-Z_][a-zA-Z0-9_]*)\(", (root / name).read_text()))
    print(summary(Path(sys.argv[1]).read_text(errors="replace"), known), end="")
