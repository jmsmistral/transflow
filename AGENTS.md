# Transflow implementation entry point

The authoritative specification is the sibling `../transflow-spec` repository. Start each implementation session with:

- `../transflow-spec/START_HERE.md` — navigation and task-scoped reading
- `../transflow-spec/AGENTS.md`

Then read the selected task, its prerequisite evidence, and the requirements, architecture sections, scenarios and decisions relevant to that change. Follow cross-references when needed; do not load every knowledge document or historical verification report by default. The canonical agreement defines staged checks, compact output and small cohesive batches.

Follow the canonical engineering agreement and the selected task's dependencies and acceptance criteria. Keep implementation and specification consistent; update the sibling knowledge documents and this repository's README when delivered behaviour changes. Do not duplicate the detailed specification or rulebook here.

If the sibling directory is absent or inaccessible, resolve that contributor setup problem rather than guessing its contents. Preserve unrelated changes in both repositories. Record actual verification results and separate repository commit references when available; do not claim tests passed, features shipped or commits exist without evidence.

The spec repository is a development requirement, not a runtime dependency of the installed application. The current baseline is specification **1.1.4**; consult its change history before implementing a superseded design.

## Root README structure

Preserve `README.md` as a concise product introduction: overview, real synthetic
visual, short authoring example, benefits, quickstart and links to the guides.
Keep its approximate 100–150-line scale. Do not append task IDs, implementation
milestones, verification totals, detailed UI change logs or internal architecture
notes. Put those in the specification task ledger, `docs/development/verification.md`
and the relevant reference guide; update `docs/README.md` when adding a guide.
See `docs/development/README.md` for contributor orientation.

## Transform authoring layout

Follow the output-first, multiline declaration style in
`python/DECLARATIONS.md#declaration-layout` for new transforms, documentation
examples and all generated sample workspaces.
