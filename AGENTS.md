# Transflow implementation entry point

The authoritative specification is the sibling `../transflow-spec` repository. Before each implementation session, read:

- `../transflow-spec/PROJECT_OVERVIEW.md`
- `../transflow-spec/TECHNICAL_ARCHITECTURE.md`
- `../transflow-spec/TASKS.md`
- `../transflow-spec/AGENTS.md`

Follow the canonical engineering agreement and the selected task's dependencies and acceptance criteria. Keep implementation and specification consistent; update the sibling knowledge documents and this repository's README when delivered behaviour changes. Do not duplicate the detailed specification or rulebook here.

If the sibling directory is absent or inaccessible, resolve that contributor setup problem rather than guessing its contents. Preserve unrelated changes in both repositories. Record actual verification results and separate repository commit references when available; do not claim tests passed, features shipped or commits exist without evidence.

The spec repository is a development requirement, not a runtime dependency of the installed application. The current baseline is specification **1.1.1**; consult its change history before implementing a superseded design.
