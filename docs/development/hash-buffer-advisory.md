# Hash-buffer advisory applicability review

Reviewed **2026-10-28** for the exact finding
`advisory:GHSA-qwgh-2vcv-g2f7:crates.io:block-buffer@0.10.4`.

## Conflicting upstream ranges

The [maintainer's advisory](https://github.com/RustCrypto/utils/security/advisories/GHSA-qwgh-2vcv-g2f7)
states that affected releases are **>=0.11.0,<0.12.1**. The
[GitHub advisory database](https://github.com/advisories/GHSA-qwgh-2vcv-g2f7)
instead lists **<0.12.1**. OSV reports the latter against our locked 0.10.4.
The complete finding is retained in `security/advisories.json`.

The issue concerns a panic overwriting a buffer cursor stored inside the data
buffer, followed by reuse after catching that panic. It is not a finding about
the SHA-256 algorithm.

## Locked-code review

Both Rust locks resolve `sha2` 0.10.9 through `digest` 0.10.7 to `block-buffer`
0.10.4. SQLx 0.9.0 also requires this SHA-2 series, so changing only Transflow's
direct hashing dependency does not remove that transitive package.

In the published 0.10.4 source, `BlockBuffer` has a separate `pos: u8` field beside
its data array. `digest_blocks` copies input only into the array and calls the
compression callback before updating the separate cursor. It cannot overwrite
that cursor with the trailing input byte used in the advisory's example.
This version has no `ReadBuffer` type from the advisory's second example.

The [native dependency probe](../../tools/qualification/rust/src/main.rs) pins
0.10.4 explicitly and adapts the maintainer's first example: fill a partial
four-byte eager buffer with `[3, 0xff]`, unwind in the compression callback,
then check that the cursor remains 2 and its existing prefix is `[1, 2]`.
It also resets/reuses the buffer and retains the SHA-256 `abc` known-answer check.
These are native regression observations, not a Miri qualification or a claim
that every operation in the dependency is verified.

## Policy decision and follow-up

The existing exact-exception mechanism records this as a **false positive for
0.10.4**, with owner `Transflow maintainers` and expiry **2026-11-18**. The finding
is not removed from audit evidence. No other advisory or package/version is
covered; affected 0.11/0.12 releases remain blocked. Expiry and unused-exception
checks remain enabled.

Review the upstream database range before expiry. Remove the exception when OSV
corrects the range or the dependency chain no longer contains 0.10.4. A later
hashing/SQLx migration still needs normal compatibility qualification.
