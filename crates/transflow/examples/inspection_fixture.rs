//! Contributor-only real retained versions for public CLI integration tests.
#[path = "../../tf-exec/tests/support/publication.rs"]
mod fixture;
use fixture::*;
use std::path::PathBuf;
use tf_domain::VersionId;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("missing synthetic workspace")?,
    );
    let configured = tf_catalog::workspace::Workspace::load(&root, Some(&root))?;
    if configured.config().id() != workspace() {
        return Err("synthetic workspace identity required".into());
    }
    runtime().block_on(async {
        let mut h = Harness::at(root).await;
        sqlx::query("UPDATE workspaces SET root_identity=? WHERE id=?")
            .bind(h.root.to_str().ok_or("fixture path is not UTF-8")?)
            .bind(workspace().to_string())
            .execute(&mut h.db)
            .await?;
        for (n, generation) in [(10, 0), (11, 1)] {
            let request = h.start(n, 10, generation, contract()).await;
            h.publish(request, n).await?;
            h.finish(n, "SUCCEEDED").await;
        }
        h.handoff().await;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    println!(
        "{}",
        serde_json::json!({"older":VersionId::from_bytes([10;16]).to_string(),"head":VersionId::from_bytes([11;16]).to_string()})
    );
    Ok(())
}
