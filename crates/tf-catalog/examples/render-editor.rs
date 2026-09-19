//! Developer fixture renderer: validated CatalogSnapshotV1 on stdin, typing files as JSON on stdout.
use std::io::{self, Read};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    io::stdin()
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    let snapshot = serde_json::from_slice(&bytes)?;
    let overlay = tf_catalog::editor::Overlay::render(&snapshot)?;
    println!("{}", serde_json::to_string_pretty(overlay.files())?);
    Ok(())
}
