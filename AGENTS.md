# Transflow implementation entry point

The authoritative specification is the sibling `../transflow-spec` repository. Start each implementation session with:

- `../transflow-spec/START_HERE.md` — navigation and task-scoped reading
- `../transflow-spec/AGENTS.md`

Then read the selected task, its prerequisite evidence, and the requirements, architecture sections, scenarios and decisions relevant to that change. Follow cross-references when needed; do not load every knowledge document or historical verification report by default. The canonical agreement defines staged checks, compact output and small cohesive batches.

Follow the canonical engineering agreement and the selected task's dependencies and acceptance criteria. Keep implementation and specification consistent; update the sibling knowledge documents and this repository's README when delivered behaviour changes. Do not duplicate the detailed specification or rulebook here.

If the sibling directory is absent or inaccessible, resolve that contributor setup problem rather than guessing its contents. Preserve unrelated changes in both repositories. Record actual verification results and separate repository commit references when available; do not claim tests passed, features shipped or commits exist without evidence.

The spec repository is a development requirement, not a runtime dependency of the installed application. The current baseline is specification **1.1.3**; consult its change history before implementing a superseded design.
