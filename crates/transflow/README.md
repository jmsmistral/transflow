# CLI diagnostics and result envelopes

The native executable currently implements help and version. It also accepts
`--json`, `--verbose`, `--color auto|always|never`, and root `--workspace DIRECTORY`.
Help/version do not discover/open a workspace, run Python or start a coordinator.
Unavailable commands and invalid options return usage status 2 with an explanation.
The common argument definitions generate the displayed help.

```bash
cargo run --locked --offline -- --help
cargo run --locked --offline -- --version --json
cargo run --locked --offline -- --workspace ./analysis --json build orders
```

The last example intentionally fails: build is not implemented. Its envelope
contains the explicitly selected consumer workspace; a later subcommand's
`--workspace` cannot replace that root context. Nearest-workspace discovery belongs
to later workspace commands. Unknown contexts are null, not guessed.

`--json` writes exactly one newline-terminated `CliEnvelopeV1` object to stdout,
with format/product versions, implemented capabilities, outcome, matching exit
status, request context, result and diagnostics. It emits no stderr/progress/ANSI
on these paths, even with `--color always`. The generated standalone schema is
[cli-result-v1.schema.json](../../schemas/generated/cli-result-v1.schema.json).
Success currently carries help or version; the domain envelope also supports
operational failures and explicit user cancellation for later services. No
asynchronous acceptance or dataset execution is advertised.

| Exit | Meaning |
|---:|---|
| 0 | Requested operation succeeded |
| 1 | Operational/validation/execution/I/O failure |
| 2 | Invalid command usage |
| 130 | Explicit interruption/cancellation of a waiting command |

Human failures go to stderr with heading, reason, affected references, locations,
remediation and safe causes. Stable codes are included by `--verbose`. Fixed ANSI
heading styling is used only in human mode when requested, or on a terminal in
auto mode. I/O errors retain their typed source internally but do not echo its raw
message; an unwritable stdout cannot be promised a complete JSON envelope.

## Domain and safety boundaries

`tf_domain::diagnostic` contains stable codes/statuses, immutable diagnostic text,
source ranges and request context. Source line/column coordinates are one-based
Unicode-scalar positions; ends are exclusive, ordered and positive. No source file
is opened. Constructors bound raw text (4096 UTF-8 bytes), sources/references (32
each) and cause trees (8 children, 4 levels, 64 total nodes). Envelope construction
adds a 128 KiB diagnostic-text budget and a final 1 MiB output cap.

All dynamic diagnostic text passes through `Redactor` before becoming `SafeText`.
Callers supply known secret values; longest matching credentials are replaced
before control escaping. Sensitive header/key lines are conservatively replaced.
C0/C1 controls, bidi controls and Unicode line separators become visible `\u{...}`
text in both human and JSON values. Even help's embedded newlines are visible
escapes in its JSON text result; the envelope itself remains ordinary valid JSON.
Safe text is display material, not a lossless original-source representation.

Raw error chains, argv values and arbitrary request dumps are never automatically
formatted. CLI parsing failures use a fixed explanation instead of echoing parser
messages. Oversized/empty explicit workspace display context gets a fixed marker.
Redaction is best effort for known values/sensitive labels, not proof that arbitrary
unknown credentials are absent. Future services must pass their configured known
secrets and never attach raw sensitive payloads as diagnostics.

The shared contract corpus includes invalid ANSI/bidi text, version/code/exit
mismatches and missing diagnostics. Actual executable tests verify stream separation,
context, flags and safe usage errors. A synthetic cycle exercises the same domain
model/renderer; graph detection and full A46/A65 command workflows remain later work.
