//! Real Parquet decoding, isolated byte copies and observable mutation/containment failures.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Synthetic integration assertions"
)]
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    os::unix::fs::{MetadataExt, symlink},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};
use tf_domain::RequestId;
use tf_store::imports::PreparedFiles;
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/tmp/tf-import-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        fs::create_dir_all(root.join(".transflow/runtime")).unwrap();
        fs::create_dir(root.join("source")).unwrap();
        Self(root)
    }
    fn put(&self, name: &str, fixture: &str) {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/import")
                .join(fixture),
            self.0.join("source").join(name),
        )
        .unwrap();
    }
    fn source(&self) -> File {
        File::open(self.0.join("source")).unwrap()
    }
    fn prepare(&self, names: &[&str]) -> PreparedFiles {
        PreparedFiles::copy(
            &self.0,
            &self.source(),
            &names.iter().map(PathBuf::from).collect::<Vec<_>>(),
            RequestId::from_bytes([3; 16]),
            |_| Ok(()),
        )
        .unwrap()
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn manifest(tree: &Tree, files: &PreparedFiles) -> Value {
    json!({"format_version":1,"kind":"import_staging","import_id":RequestId::from_bytes([3;16]).to_string(),"workspace_id":RequestId::from_bytes([1;16]).to_string(),"dataset_id":RequestId::from_bytes([2;16]).to_string(),"path":"raw/input","branch":"master","source_snapshot_id":RequestId::from_bytes([4;16]).to_string(),"schema_normalization":"pending","source_snapshot_limitation":true,"catalog_fingerprint":"a".repeat(64),"environment_fingerprint":"b".repeat(64),"validation_certificate_fingerprint":"c".repeat(64),"source_root":tree.0.join("source"),"source_files":["a.parquet"],"files":files.files()})
}
#[test]
fn copied_parquet_is_independent_and_zero_rows_are_valid() {
    for (fixture, rows) in [("empty.parquet", 0), ("two-rows.parquet", 2)] {
        let tree = Tree::new();
        tree.put("a.parquet", fixture);
        let prepared = tree.prepare(&["a.parquet"]);
        assert_eq!(prepared.rows().unwrap(), rows);
        let metadata = manifest(&tree, &prepared);
        let path = prepared.retain(&metadata).unwrap();
        let copied = path.join("part-00000.parquet");
        let before = fs::read(&copied).unwrap();
        assert_ne!(
            fs::metadata(&copied).unwrap().ino(),
            fs::metadata(tree.0.join("source/a.parquet")).unwrap().ino()
        );
        fs::write(
            tree.0.join("source/a.parquet"),
            b"mutable original replaced",
        )
        .unwrap();
        assert_eq!(fs::read(copied).unwrap(), before);
        assert!(path.join("prepared.json").is_file());
    }
}
#[test]
fn corrupt_footer_source_changes_and_staged_tampering_are_refused() {
    let tree = Tree::new();
    tree.put("a.parquet", "two-rows.parquet");
    let id = RequestId::from_bytes([3; 16]);
    let result = PreparedFiles::copy(&tree.0, &tree.source(), &["a.parquet".into()], id, |_| {
        fs::write(
            tree.0.join("source/a.parquet"),
            b"changed during preparation",
        )
    });
    assert!(result.is_err());
    assert_eq!(
        fs::read_dir(tree.0.join(".transflow/runtime/import-staging"))
            .unwrap()
            .count(),
        0
    );
    assert!(
        PreparedFiles::copy(&tree.0, &tree.source(), &["a.parquet".into()], id, |_| Ok(
            ()
        ))
        .is_err()
    );
    tree.put("a.parquet", "two-rows.parquet");
    let prepared = tree.prepare(&["a.parquet"]);
    fs::write(
        tree.0.join(format!(
            ".transflow/runtime/import-staging/.staging-{id}/part-00000.parquet"
        )),
        b"staged bytes changed",
    )
    .unwrap();
    assert!(prepared.verify_copies().is_err());
    assert!(prepared.retain(&json!({})).is_err());
}
#[test]
fn links_and_unlisted_staging_members_cannot_be_retained() {
    let tree = Tree::new();
    tree.put("a.parquet", "two-rows.parquet");
    symlink(
        tree.0.join("source/a.parquet"),
        tree.0.join("source/link.parquet"),
    )
    .unwrap();
    assert!(
        PreparedFiles::copy(
            &tree.0,
            &tree.source(),
            &["link.parquet".into()],
            RequestId::from_bytes([3; 16]),
            |_| Ok(())
        )
        .is_err()
    );
    let prepared = tree.prepare(&["a.parquet"]);
    let id = RequestId::from_bytes([3; 16]);
    fs::write(
        tree.0.join(format!(
            ".transflow/runtime/import-staging/.staging-{id}/unlisted"
        )),
        b"unexpected",
    )
    .unwrap();
    assert!(prepared.verify_copies().is_err());
}

#[test]
fn different_file_schemas_are_refused_without_retained_staging() {
    use arrow_array::{Int64Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use parquet::arrow::ArrowWriter;
    use std::sync::Arc;
    let tree = Tree::new();
    tree.put("a.parquet", "two-rows.parquet");
    let schema = Arc::new(Schema::new(vec![Field::new(
        "different",
        DataType::Int64,
        false,
    )]));
    let batch =
        RecordBatch::try_new(schema.clone(), vec![Arc::new(Int64Array::from(vec![3i64]))]).unwrap();
    let mut writer = ArrowWriter::try_new(
        File::create(tree.0.join("source/b.parquet")).unwrap(),
        schema,
        None,
    )
    .unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    assert!(
        PreparedFiles::copy(
            &tree.0,
            &tree.source(),
            &["a.parquet".into(), "b.parquet".into()],
            RequestId::from_bytes([3; 16]),
            |_| Ok(())
        )
        .is_err()
    );
    assert_eq!(
        fs::read_dir(tree.0.join(".transflow/runtime/import-staging"))
            .unwrap()
            .count(),
        0
    );
}
