//! Human presentation only. Machine protocols and exported payloads bypass this module.
use serde_json::Value;
use std::io::{self, IsTerminal, Write};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Style {
    pub rich: bool,
    pub color: bool,
    pub unicode: bool,
}
impl Style {
    pub fn stdout(choice: Option<&str>) -> Self {
        Self::detect(choice, io::stdout().is_terminal())
    }
    pub fn stderr(choice: Option<&str>) -> Self {
        Self::detect(choice, io::stderr().is_terminal())
    }
    fn detect(choice: Option<&str>, tty: bool) -> Self {
        let dumb = std::env::var("TERM").is_ok_and(|s| s == "dumb");
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|s| !s.is_empty());
        let color = match choice {
            Some("always") => true,
            Some("never") => false,
            _ => tty && !dumb && !no_color,
        };
        let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .filter_map(|key| std::env::var(key).ok())
            .find(|s| !s.is_empty())
            .unwrap_or_default()
            .to_ascii_lowercase();
        Self {
            rich: tty && !dumb || color,
            color,
            unicode: locale.contains("utf"),
        }
    }
    pub fn paint(self, text: &str, code: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.into()
        }
    }
    pub fn status(self, state: &str) -> String {
        let (label, code, symbol) = match state {
            "SUCCEEDED" => ("Succeeded", "32", if self.unicode { "✓" } else { "+" }),
            "CACHED" => ("Cached", "32", if self.unicode { "✓" } else { "+" }),
            "FAILED" => ("Failed", "31", if self.unicode { "✗" } else { "x" }),
            "BLOCKED" => ("Blocked", "31", "!"),
            "CANCELED" => ("Canceled", "33", "-"),
            "INTERRUPTED" => ("Interrupted", "33", "!"),
            "RETRY_WAIT" => ("Retrying", "33", "~"),
            "WAITING_DEPENDENCIES" => ("Waiting for inputs", "90", "·"),
            "PLANNED" => ("Planned", "90", "·"),
            "QUEUED" => ("Queued", "90", "·"),
            "ACCEPTED" => ("Accepted", "36", ">"),
            "STARTING" => ("Starting", "36", ">"),
            "VALIDATING_INPUTS" => ("Checking inputs", "36", ">"),
            "RUNNING" => ("Running", "36", ">"),
            "MATERIALIZING" => ("Writing data", "36", ">"),
            "VALIDATING_OUTPUTS" => ("Checking output", "36", ">"),
            "COMMITTING" => ("Publishing", "36", ">"),
            _ => return safe(state),
        };
        let symbol = if !self.unicode && symbol == "·" {
            "."
        } else {
            symbol
        };
        self.paint(&format!("{symbol} {label}"), code)
    }
    /// Keep existing plain text stable for pipes; style and space human terminal reports.
    pub fn write(self, writer: &mut dyn Write, text: &str) -> io::Result<()> {
        if !self.rich {
            return writeln!(writer, "{text}");
        }
        let text = safe(text);
        // Some older report adapters return JSON in human mode. Render fields, not JSON syntax.
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            return writeln!(writer, "\n{}", self.record(&value, 0));
        }
        writeln!(writer)?;
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            let styled = if index == 0
                || (text.starts_with("  _                       __ _") && index < 5)
                || trimmed.ends_with(':')
                || ["Get started", "CLI reference", "Bash completion"]
                    .iter()
                    .any(|heading| trimmed.starts_with(heading))
            {
                self.paint(line, "1;36")
            } else if trimmed.starts_with("Warning:") {
                self.paint(line, "33")
            } else if trimmed.starts_with("Blocked:") {
                self.paint(line, "31")
            } else if let Some((key, value)) = line.split_once(": ") {
                format!("{} {value}", self.paint(&format!("{key}:"), "2"))
            } else {
                line.into()
            };
            writeln!(writer, "  {styled}")?;
        }
        writeln!(writer)
    }
    pub fn record(self, value: &Value, depth: usize) -> String {
        let pad = "  ".repeat(depth.min(24));
        if depth > 24 {
            return format!("{pad}[nested detail: use --json]\n");
        }
        match value {
            Value::Object(fields) => fields
                .iter()
                .map(|(key, value)| {
                    let label = safe(key).replace('_', " ");
                    if value.is_object() || value.is_array() {
                        format!(
                            "{pad}{}\n{}",
                            self.paint(&label, "1"),
                            self.record(value, depth + 1)
                        )
                    } else {
                        format!("{pad}{}  {}\n", self.paint(&label, "2"), self.scalar(value))
                    }
                })
                .collect(),
            Value::Array(items) if items.is_empty() => format!("{pad}None\n"),
            Value::Array(items) => items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    if item.is_object() || item.is_array() {
                        format!(
                            "{pad}{}\n{}",
                            self.paint(&format!("{}.", i + 1), "36"),
                            self.record(item, depth + 1)
                        )
                    } else {
                        format!("{pad}- {}\n", self.scalar(item))
                    }
                })
                .collect(),
            _ => format!("{pad}{}\n", self.scalar(value)),
        }
    }
    fn scalar(self, value: &Value) -> String {
        match value {
            Value::Null => self.paint("—", "2"),
            Value::Bool(true) => "Yes".into(),
            Value::Bool(false) => "No".into(),
            Value::String(s) if terminal_state(s) || active_state(s) => self.status(s),
            Value::String(s) => safe(s),
            _ => value.to_string(),
        }
    }
    pub fn table(self, headers: &[&str], rows: &[Vec<String>], width: usize) -> String {
        if rows.is_empty() {
            return "  No entries.\n".into();
        }
        let columns = headers.len().max(1);
        let budget = width.saturating_sub(2 + (columns - 1) * 2);
        let mut widths: Vec<_> = headers
            .iter()
            .enumerate()
            .map(|(i, h)| {
                rows.iter()
                    .filter_map(|r| r.get(i))
                    .map(|s| cells(s))
                    .max()
                    .unwrap_or(0)
                    .max(cells(h))
            })
            .collect();
        while widths.iter().sum::<usize>() > budget {
            let Some((index, widest)) = widths.iter().enumerate().max_by_key(|(_, width)| **width)
            else {
                break;
            };
            if *widest <= 1 {
                break;
            }
            widths[index] -= 1;
        }
        let line = |values: Vec<String>| -> String {
            values
                .iter()
                .zip(&widths)
                .map(|(v, width)| {
                    let v = clip(v, *width);
                    format!("{v}{}", " ".repeat(width.saturating_sub(cells(&v))))
                })
                .collect::<Vec<_>>()
                .join("  ")
        };
        let mut out = format!(
            "  {}\n",
            self.paint(&line(headers.iter().map(|s| (*s).into()).collect()), "1;2")
        );
        for row in rows {
            let mut text = line(row.clone());
            for state in [
                "SUCCEEDED",
                "CACHED",
                "FAILED",
                "BLOCKED",
                "CANCELED",
                "INTERRUPTED",
                "RUNNING",
                "QUEUED",
            ] {
                let code = if matches!(state, "SUCCEEDED" | "CACHED") {
                    "32"
                } else if matches!(state, "FAILED" | "BLOCKED") {
                    "31"
                } else {
                    "36"
                };
                text = text.replace(state, &self.paint(state, code));
            }
            out.push_str(&format!("  {text}\n"));
        }
        out
    }
}
pub(crate) fn terminal_state(s: &str) -> bool {
    matches!(
        s,
        "SUCCEEDED" | "CACHED" | "FAILED" | "BLOCKED" | "CANCELED" | "INTERRUPTED"
    )
}
pub(crate) fn active_state(s: &str) -> bool {
    matches!(
        s,
        "STARTING"
            | "VALIDATING_INPUTS"
            | "RUNNING"
            | "MATERIALIZING"
            | "VALIDATING_OUTPUTS"
            | "COMMITTING"
            | "RETRY_WAIT"
    )
}
pub(crate) fn safe(text: &str) -> String {
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
// A conservative width for non-ASCII labels prevents wide glyphs wrapping live rows.
pub(crate) fn cells(text: &str) -> usize {
    text.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}
pub(crate) fn clip(text: &str, width: usize) -> String {
    let text = safe(text).replace(['\n', '\t', '\r'], " ");
    if cells(&text) <= width {
        return text;
    }
    let mut result = String::new();
    for c in text.chars() {
        if cells(&result) + if c.is_ascii() { 1 } else { 2 } > width.saturating_sub(3) {
            break;
        }
        result.push(c);
    }
    result.push_str(&".".repeat(width.min(3)));
    result
}
pub(crate) fn dimensions(stderr: bool) -> (usize, usize) {
    let size = if stderr {
        rustix::termios::tcgetwinsize(io::stderr())
    } else {
        rustix::termios::tcgetwinsize(io::stdout())
    };
    size.ok()
        .filter(|s| s.ws_col > 0 && s.ws_row > 0)
        .map(|s| (usize::from(s.ws_col), usize::from(s.ws_row)))
        .unwrap_or((100, 24))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn untrusted_terminal_controls_are_escaped_and_widths_are_bounded() {
        let value = "dataset\u{1b}[2J\r\u{202e}data";
        let sanitized = safe(value);
        assert!(!sanitized.contains('\x1b'));
        assert!(!sanitized.contains('\r'));
        assert!(sanitized.contains("\\u{1b}"));
        let record = Style::default().record(&serde_json::json!({"\u{1b}[2J": "\u{1b}[31m"}), 0);
        assert!(!record.contains('\x1b'));
        for width in 0..60 {
            assert!(cells(&clip(value, width)) <= width);
            assert!(cells(&clip("日本語/数据 with words", width)) <= width);
        }
    }
    #[test]
    fn plain_reports_and_uncoloured_statuses_remain_readable() -> io::Result<()> {
        let mut bytes = Vec::new();
        Style::default().write(&mut bytes, "Plan\nWarning: existing contract")?;
        assert_eq!(bytes, b"Plan\nWarning: existing contract\n");
        let rich = Style {
            rich: true,
            ..Default::default()
        };
        assert_eq!(rich.status("SUCCEEDED"), "+ Succeeded");
        assert_eq!(rich.status("CANCELED"), "- Canceled");
        assert_eq!(rich.status("RETRY_WAIT"), "~ Retrying");
        let text = rich.table(
            &["DATASET", "STATUS"],
            &[vec!["a long dataset name".into(), "SUCCEEDED".into()]],
            28,
        );
        assert!(!text.contains('\x1b'));
        assert!(text.lines().all(|line| cells(line) <= 28));
        Ok(())
    }
}
