//! Bounded retained log reads. Filenames derive from validated attempt IDs, never client paths.
use crate::build_plan::{Error, failure};
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::Duration,
};
use tf_domain::{BuildId, WorkspaceId};
pub(crate) fn execute(
    root: &Path,
    workspace: WorkspaceId,
    build: BuildId,
    follow: bool,
    json_mode: bool,
) -> Result<crate::build_cli::Report, Error> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(failure)?;
    let mut offsets = BTreeMap::new();
    let mut chunks = vec![];
    let mut total = 0usize;
    let mut truncated = false;
    loop {
        let value = rt.block_on(crate::build_cli::snapshot(root, workspace, build))?;
        for job in value["jobs"]
            .as_array()
            .ok_or_else(|| failure("missing jobs"))?
        {
            for attempt in job["attempts"]
                .as_array()
                .ok_or_else(|| failure("missing attempts"))?
            {
                let id = attempt["id"]
                    .as_str()
                    .ok_or_else(|| failure("missing attempt"))?
                    .parse::<tf_domain::AttemptId>()
                    .map_err(failure)?;
                let directory = root
                    .join(".transflow/runtime/attempts")
                    .join(id.to_string());
                if !directory.try_exists().map_err(failure)? {
                    continue;
                }
                if std::fs::canonicalize(&directory).map_err(failure)? != directory {
                    return Err(failure("unsafe log directory"));
                }
                let mut helpers = std::fs::read_dir(&directory)
                    .map_err(failure)?
                    .collect::<std::io::Result<Vec<_>>>()
                    .map_err(failure)?;
                helpers.sort_by_key(|e| e.file_name());
                for helper in helpers {
                    if !helper.file_type().map_err(failure)?.is_dir() {
                        return Err(failure("unsafe log helper"));
                    }
                    for stream in ["stdout.log", "stderr.log"] {
                        let path = helper.path().join(stream);
                        if !path.try_exists().map_err(failure)? {
                            continue;
                        }
                        let meta = std::fs::symlink_metadata(&path).map_err(failure)?;
                        if !meta.is_file()
                            || meta.is_symlink()
                            || std::fs::canonicalize(&path).map_err(failure)? != path
                        {
                            return Err(failure("unsafe retained log"));
                        }
                        let offset = offsets.entry(path.clone()).or_insert(0u64);
                        let remaining = (16 * 1024 * 1024usize).saturating_sub(total);
                        let mut file = tf_exec::retained_logs::open(&path).map_err(failure)?;
                        file.seek(SeekFrom::Start(*offset)).map_err(failure)?;
                        let mut bytes = vec![];
                        file.take(remaining as u64)
                            .read_to_end(&mut bytes)
                            .map_err(failure)?;
                        *offset += bytes.len() as u64;
                        total += bytes.len();
                        truncated |= *offset < meta.len();
                        if !bytes.is_empty() {
                            let text = String::from_utf8_lossy(&bytes).into_owned();
                            if follow && !json_mode {
                                eprint!(
                                    "[{} / {} / {}] {}",
                                    job["dataset"].as_str().unwrap_or(""),
                                    id,
                                    stream,
                                    safe(&text)
                                );
                            }
                            chunks.push(json!({"dataset":job["dataset"],"attempt":id.to_string(),"helper":helper.file_name().to_string_lossy(),"stream":stream,"text":text}));
                        }
                    }
                }
            }
        }
        if !follow || !matches!(value["state"].as_str(), Some("QUEUED" | "RUNNING")) || truncated {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut report = crate::build_cli::report(
        "logs",
        json!({"id":build.to_string(),"chunks":chunks,"truncated":truncated}),
        false,
    );
    report.human = if follow {
        "Log stream complete.".into()
    } else {
        chunks
            .iter()
            .map(|c| {
                format!(
                    "[{} / {} / {} / {}]\n{}",
                    c["dataset"].as_str().unwrap_or(""),
                    c["attempt"].as_str().unwrap_or(""),
                    c["helper"].as_str().unwrap_or(""),
                    c["stream"].as_str().unwrap_or(""),
                    safe(c["text"].as_str().unwrap_or(""))
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    if truncated {
        report.human.push_str("\n[transflow: combined log display truncated at 16 MiB; retained attempt logs remain available]");
    }
    Ok(report)
}
fn safe(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c != '\n' && c != '\t' && tf_domain::diagnostic::unsafe_character(c) {
                format!("\\u{{{:x}}}", u32::from(c))
                    .chars()
                    .collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}
