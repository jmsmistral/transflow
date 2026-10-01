//! Plan-bound, bounded retained log excerpts; no caller filesystem paths.
use crate::api_read::{Result, bad};
use serde_json::{Value, json};
use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};
use tf_api::{ApiError as E, Request};
use tf_domain::AttemptId;

pub(crate) fn read(root: &Path, attempt: AttemptId, request: &Request) -> Result<Value> {
    let directory = root
        .join(".transflow/runtime/attempts")
        .join(attempt.to_string());
    let mut streams = vec![];
    if directory.try_exists().map_err(bad)? {
        if std::fs::canonicalize(&directory).map_err(bad)? != directory {
            return Err(E::missing());
        }
        for (index, helper) in std::fs::read_dir(&directory).map_err(bad)?.enumerate() {
            if index >= 128 {
                return Err(E::new(
                    413,
                    "TF_API_LOG_LIMIT",
                    "Too many retained log helpers",
                ));
            }
            let helper = helper.map_err(bad)?;
            if !helper.file_type().map_err(bad)?.is_dir() {
                return Err(E::missing());
            }
            let name = helper.file_name().into_string().map_err(bad)?;
            if name.len() > 128 || streams.len() >= 256 {
                return Err(E::new(
                    413,
                    "TF_API_LOG_LIMIT",
                    "Too many retained log streams",
                ));
            }
            for channel in ["stdout", "stderr"] {
                let path = helper.path().join(format!("{channel}.log"));
                if !path.try_exists().map_err(bad)? {
                    continue;
                }
                let file = tf_exec::retained_logs::open(&path).map_err(|_| E::missing())?;
                streams.push(json!({"helper":name,"stream":channel,"bytes":file.metadata().map_err(bad)?.len().to_string()}));
            }
        }
        streams.sort_by(|a, b| {
            (a["helper"].as_str(), a["stream"].as_str())
                .cmp(&(b["helper"].as_str(), b["stream"].as_str()))
        });
    }
    let selected = if let Some(helper) = request.query.get("helper") {
        let channel = request.query.get("stream").ok_or_else(E::invalid)?;
        Some(
            streams
                .iter()
                .find(|s| s["helper"] == *helper && s["stream"] == *channel)
                .ok_or_else(E::missing)?,
        )
    } else {
        if request.query.contains_key("stream") {
            return Err(E::invalid());
        }
        streams.first()
    };
    let offset = request
        .query
        .get("offset")
        .map(|s| s.parse::<u64>().map_err(|_| E::invalid()))
        .transpose()?
        .unwrap_or(0);
    let limit = request
        .query
        .get("limit")
        .map(|s| s.parse::<u64>().map_err(|_| E::invalid()))
        .transpose()?
        .unwrap_or(32768);
    if !(1..=32768).contains(&limit) {
        return Err(E::invalid());
    }
    let mut text = String::new();
    let mut next = None;
    if let Some(s) = selected {
        let helper = s["helper"].as_str().ok_or_else(E::internal)?;
        let channel = s["stream"].as_str().ok_or_else(E::internal)?;
        let path = directory.join(helper).join(format!("{channel}.log"));
        let mut file = tf_exec::retained_logs::open(&path).map_err(|_| E::missing())?;
        let length = file.metadata().map_err(bad)?.len();
        if offset > length {
            return Err(E::invalid());
        }
        file.seek(SeekFrom::Start(offset)).map_err(bad)?;
        let mut bytes = vec![];
        file.take(limit).read_to_end(&mut bytes).map_err(bad)?;
        let end = offset + bytes.len() as u64;
        next = (end < length).then(|| end.to_string());
        text = String::from_utf8_lossy(&bytes).into_owned();
    } else if offset != 0 {
        return Err(E::invalid());
    }
    Ok(
        json!({"attempt":attempt.to_string(),"streams":streams,"helper":selected.map(|s|s["helper"].clone()),"stream":selected.map(|s|s["stream"].clone()),"offset":offset.to_string(),"text":text,"next_offset":next,"available":selected.is_some()}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_logs_refuse_paths_and_symlinks()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("tf-api-logs-{}", std::process::id()));
        let root = {
            std::fs::create_dir_all(&root)?;
            std::fs::canonicalize(root)?
        };
        let attempt: AttemptId = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".parse()?;
        let helper = root
            .join(".transflow/runtime/attempts")
            .join(attempt.to_string())
            .join("worker");
        std::fs::create_dir_all(&helper)?;
        std::fs::write(helper.join("stdout.log"), "one\ntwo\n")?;
        let mut r = Request {
            id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".parse()?,
            method: "GET".into(),
            path: "".into(),
            query: Default::default(),
            body: Value::Null,
            key: None,
            if_match: None,
        };
        r.query.insert("limit".into(), "4".into());
        let page = read(&root, attempt, &r)?;
        assert_eq!(page["text"], "one\n");
        assert_eq!(page["next_offset"], "4");
        r.query.insert("offset".into(), "4".into());
        assert_eq!(read(&root, attempt, &r)?["text"], "two\n");
        r.query.insert("helper".into(), "../worker".into());
        r.query.insert("stream".into(), "stdout".into());
        assert!(read(&root, attempt, &r).is_err());
        r.query.clear();
        std::fs::remove_file(helper.join("stdout.log"))?;
        std::os::unix::fs::symlink("/etc/passwd", helper.join("stdout.log"))?;
        assert!(read(&root, attempt, &r).is_err());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
