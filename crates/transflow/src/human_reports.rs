//! Curated terminal summaries. Complete retained evidence stays available via --json.
use crate::{
    build_progress,
    terminal::{self, Style},
};
use serde_json::Value;
use std::io::{self, Write};

pub(crate) fn write(
    writer: &mut dyn Write,
    style: Style,
    command: &str,
    value: &Value,
    fallback: &str,
) -> io::Result<()> {
    let data = &value["data"];
    let operation = value["operation"].as_str().unwrap_or("");
    if command == "build" && operation == "logs" {
        return writeln!(writer, "{}", terminal::safe(fallback));
    }
    if command == "build" && data["jobs"].is_array() {
        writeln!(
            writer,
            "\n  {}  {}",
            style.paint("Build", "1"),
            style.status(data["state"].as_str().unwrap_or("UNKNOWN"))
        )?;
        writeln!(
            writer,
            "  {}",
            style.paint(&terminal::safe(data["id"].as_str().unwrap_or("")), "2")
        )?;
        let branch = data["plan"]["output"]["name"].as_str().unwrap_or("unknown");
        writeln!(
            writer,
            "  Branch {}  ·  elapsed {}\n",
            terminal::safe(branch),
            duration(data["created_us"].as_str(), data["finished_us"].as_str())
        )?;
        let labels = build_progress::names(data);
        let mut totals = std::collections::BTreeMap::new();
        let order: std::collections::BTreeMap<_, _> = data["plan"]["writes"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(i, w)| w["job"].as_str().map(|id| (id, i)))
            .collect();
        let mut jobs: Vec<_> = data["jobs"].as_array().into_iter().flatten().collect();
        jobs.sort_by_key(|j| {
            order
                .get(j["id"].as_str().unwrap_or(""))
                .copied()
                .unwrap_or(usize::MAX)
        });
        for job in jobs {
            let state = job["state"].as_str().unwrap_or("UNKNOWN");
            *totals.entry(state).or_insert(0usize) += 1;
            let id = job["dataset"].as_str().unwrap_or("unknown");
            let name = terminal::safe(labels.get(id).map(String::as_str).unwrap_or(id));
            writeln!(
                writer,
                "  {}  {}",
                style.status(state),
                style.paint(&name, "1")
            )?;
            let attempts = job["attempts"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            if operation == "show" {
                if let Some(version) = job["version"].as_str() {
                    writeln!(writer, "      Version  {}", terminal::safe(version))?;
                }
                for attempt in attempts {
                    writeln!(
                        writer,
                        "      Attempt {}  {}  {}",
                        attempt["number"],
                        style.status(attempt["state"].as_str().unwrap_or("")),
                        duration(
                            attempt["started_us"].as_str(),
                            attempt["finished_us"].as_str()
                        )
                    )?;
                    if let Some(failure) = attempt["failure_class"].as_str() {
                        writeln!(
                            writer,
                            "      {}",
                            style.paint(&terminal::safe(failure), "31")
                        )?;
                    }
                    for phase in attempt["phases"].as_array().into_iter().flatten() {
                        writeln!(
                            writer,
                            "        {}  {}",
                            terminal::safe(phase["phase"].as_str().unwrap_or("")),
                            duration(phase["started_us"].as_str(), phase["finished_us"].as_str())
                        )?;
                    }
                }
            }
            let checks = attempts
                .iter()
                .flat_map(|a| a["checks"].as_array().into_iter().flatten())
                .chain(
                    job["cached_from"]["checks"]
                        .as_array()
                        .into_iter()
                        .flatten(),
                );
            for check in checks {
                let outcome = check["outcome"].as_str().unwrap_or("Unknown");
                if operation == "show" || outcome != "PASS" {
                    let warn = check["policy"]["severity"] == "WARN";
                    let code = if outcome == "PASS" {
                        "32"
                    } else if warn {
                        "33"
                    } else {
                        "31"
                    };
                    writeln!(
                        writer,
                        "      {}  {}",
                        style.paint(
                            &terminal::safe(if warn && outcome == "VIOLATION" {
                                "Warning"
                            } else {
                                outcome
                            }),
                            code
                        ),
                        terminal::safe(check["name"].as_str().unwrap_or("Check"))
                    )?;
                }
            }
        }
        writeln!(
            writer,
            "\n  {}",
            totals
                .iter()
                .map(|(s, n)| format!("{n} {}", s.to_ascii_lowercase()))
                .collect::<Vec<_>>()
                .join(" · ")
        )?;
        writeln!(
            writer,
            "  {}\n",
            style.paint(
                "Use build show <id> for details; --json for complete execution evidence.",
                "2"
            )
        )
    } else if command == "build" && data["builds"].is_array() {
        writeln!(writer, "\n  {}\n", style.paint("Build history", "1;36"))?;
        let rows = data["builds"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|b| {
                vec![
                    field(b, "id"),
                    field(b, "state"),
                    field(b, "branch"),
                    duration(b["created_us"].as_str(), b["finished_us"].as_str()),
                ]
            })
            .collect::<Vec<_>>();
        write!(
            writer,
            "{}",
            style.table(
                &["BUILD", "STATUS", "BRANCH", "ELAPSED"],
                &rows,
                terminal::dimensions(false).0
            )
        )?;
        if let Some(cursor) = data["next_cursor"].as_str() {
            writeln!(writer, "\n  Next cursor: {}", terminal::safe(cursor))?;
        }
        writeln!(writer)
    } else if command == "build" && data["state"] == "ACCEPTED" {
        writeln!(
            writer,
            "\n  {}\n  {}\n\n  Build accepted. Execution continues in the coordinator.\n  transflow build show {}\n",
            style.status("ACCEPTED"),
            field(data, "id"),
            field(data, "id")
        )
    } else if command == "schedule" && data["schedules"].is_array() {
        let rows = data["schedules"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| {
                vec![
                    field(s, "id"),
                    field(&s["definition"], "name"),
                    if s["needs_review"] == true {
                        "Needs review".into()
                    } else if s["paused"] == true {
                        "Paused".into()
                    } else if s["currently_running"] == true {
                        "Running".into()
                    } else {
                        "Active".into()
                    },
                    field(&s["definition"]["build"], "data_branch"),
                ]
            })
            .collect::<Vec<_>>();
        listing(
            writer,
            style,
            "Schedules",
            &["ID", "NAME", "STATUS", "BRANCH"],
            &rows,
            data,
        )
    } else {
        if command == "serve" && data["state"] == "STOPPED" {
            return style.write(writer, "Coordinator stopped.");
        }
        writeln!(
            writer,
            "\n  {}\n",
            style.paint(&format!("{} {}", title(command), title(operation)), "1;36")
        )?;
        writeln!(writer, "{}", style.record(data, 1))
    }
}
fn field(value: &Value, key: &str) -> String {
    let text = value[key].as_str().unwrap_or("-");
    if text.is_empty() {
        return "-".into();
    }
    tf_domain::diagnostic::Redactor::default()
        .text(text)
        .map(|safe| safe.as_str().to_owned())
        .unwrap_or_else(|_| "[field exceeds display limit; use --json]".into())
}
fn title(text: &str) -> String {
    let text = terminal::safe(text).replace('_', " ");
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => text,
    }
}
fn duration(start: Option<&str>, end: Option<&str>) -> String {
    start
        .and_then(|s| s.parse::<u64>().ok())
        .zip(end.and_then(|s| s.parse::<u64>().ok()))
        .and_then(|(s, e)| e.checked_sub(s))
        .map(|us| format!("{:.2}s", us as f64 / 1_000_000.))
        .unwrap_or_else(|| {
            if start.is_some() && end.is_none() {
                "in progress".into()
            } else {
                "unavailable".into()
            }
        })
}

pub(crate) fn registry(
    writer: &mut dyn Write,
    style: Style,
    command: &str,
    value: &Value,
    fallback: &str,
) -> io::Result<()> {
    if !style.rich || value["operation"] != "list" {
        return style.write(writer, fallback);
    }
    let entries = value["entries"].as_array().into_iter().flatten();
    let (heading, headers, rows) = if command == "catalog" {
        (
            "Catalogue",
            ["DATASET", "KIND", "VERSIONS", "BRANCH"],
            entries
                .map(|e| {
                    vec![
                        field(e, "path"),
                        field(e, "kind"),
                        e["version_count"]
                            .as_str()
                            .unwrap_or("not inspected")
                            .into(),
                        field(e, "branch_context"),
                    ]
                })
                .collect::<Vec<_>>(),
        )
    } else {
        (
            "Data branches",
            ["BRANCH", "HEADS", "FALLBACK ORDER", "STATUS"],
            entries
                .map(|e| {
                    vec![
                        field(e, "name"),
                        field(e, "heads"),
                        e["fallbacks"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" -> "),
                        if e["deleted"] == true {
                            "Deleted".into()
                        } else {
                            "Active".into()
                        },
                    ]
                })
                .collect::<Vec<_>>(),
        )
    };
    listing(writer, style, heading, &headers, &rows, value)?;
    writeln!(
        writer,
        "  {}\n",
        style.paint(
            &format!(
                "Use {command} {} for details; --json for full identifiers and fields.",
                if command == "catalog" {
                    "show <dataset>"
                } else {
                    "list --json"
                }
            ),
            "2"
        )
    )
}
fn listing(
    writer: &mut dyn Write,
    style: Style,
    heading: &str,
    headers: &[&str],
    rows: &[Vec<String>],
    value: &Value,
) -> io::Result<()> {
    writeln!(
        writer,
        "\n  {}  {}\n",
        style.paint(heading, "1;36"),
        style.paint(&format!("{} entries", rows.len()), "2")
    )?;
    write!(
        writer,
        "{}",
        style.table(headers, rows, terminal::dimensions(false).0)
    )?;
    if let Some(cursor) = value["next_cursor"].as_str() {
        writeln!(writer, "\n  Next cursor: {}", terminal::safe(cursor))?;
    }
    writeln!(writer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn summary_keeps_warning_evidence_and_uses_branch_name() -> io::Result<()> {
        let data = json!({"operation":"run", "data":{
            "id":"test-build", "state":"SUCCEEDED", "created_us":"1000000", "finished_us":"3000000",
            "plan":{"output":{"name":"review", "branch":"branch-uuid"}},
            "jobs":[{"dataset":"curated/orders", "state":"SUCCEEDED", "attempts":[{
                "checks":[{"outcome":"VIOLATION", "policy":{"severity":"WARN"}, "name":"amounts"}]
            }]}]
        }});
        let mut bytes = Vec::new();
        write(
            &mut bytes,
            Style {
                rich: true,
                ..Default::default()
            },
            "build",
            &data,
            "",
        )?;
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("Branch review"));
        assert!(!text.contains("branch-uuid"));
        assert!(text.contains("2.00s"));
        assert!(text.contains("Warning  amounts"));
        assert!(text.contains("+ Succeeded"));
        assert!(!text.contains('\x1b'));
        Ok(())
    }
    #[test]
    fn log_text_is_not_reformatted_as_json_or_report_headings() -> io::Result<()> {
        let mut bytes = Vec::new();
        let text = "{\"user\": \"literal log\"}\nWarning: a user's words\n\x1b[2J";
        write(
            &mut bytes,
            Style {
                rich: true,
                color: true,
                unicode: true,
            },
            "build",
            &json!({"operation":"logs"}),
            text,
        )?;
        assert_eq!(
            String::from_utf8_lossy(&bytes),
            format!("{}\n", terminal::safe(text))
        );
        Ok(())
    }
}
