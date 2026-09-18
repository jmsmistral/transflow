# Python source boundary

T005 will establish separate typed SDK and worker distributions under `sdk/`
and `worker/`, after T003 qualifies dependencies. Their process and packaging
contracts live in the [canonical architecture](../../transflow-spec/TECHNICAL_ARCHITECTURE.md).

No installable Python package exists yet. Use the project's pyenv environment
for contributor tools; those tools currently need only the standard library.
