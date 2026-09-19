# Synthetic Parquet import fixtures

These repository-owned files contain only an `id: int64` column: zero rows in
`empty.parquet` and values 1, 2 in `two-rows.parquet`. No user data is included.
Regenerate explicitly with the qualified Arrow/Parquet 60.0.0 toolchain:

```bash
cargo run --locked --offline -p tf-store --example import_fixtures -- tests/fixtures/import
```

Import tests decode every row, verify zero-row acceptance, compare copied bytes,
and assert that source/staging inodes are distinct. Normalization and publication
are separate later stages.

`unsupported-duration.parquet` is a valid synthetic duration column that the portable logical contract must reject before import registration.
