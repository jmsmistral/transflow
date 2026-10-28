//! Bounded, observational build display shared by local execution and attached clients.
use crate::terminal::{self, Style, active_state, clip, terminal_state};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Write},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Default)]
pub(crate) struct Options {
    pub enabled: bool,
    pub live: bool,
    pub style: Style,
}
impl Options {
    pub fn cli(json: bool, color: Option<&str>) -> Self {
        Self {
            enabled: !json,
            live: !json
                && io::stdout().is_terminal()
                && io::stderr().is_terminal()
                && std::env::var("TERM").is_ok_and(|s| s != "dumb")
                && std::env::var_os("CI").is_none(),
            style: Style::stderr(color),
        }
    }
    pub fn server(json: bool) -> Self {
        Self {
            enabled: !json,
            ..Default::default()
        }
    }
}
struct Job {
    id: String,
    name: String,
    state: String,
    since: Instant,
    elapsed: Duration,
}
pub(crate) struct Display<W: Write> {
    writer: W,
    options: Options,
    build: String,
    jobs: Vec<Job>,
    started: Instant,
    drawn: usize,
    width: usize,
    frame: usize,
    last_frame: Option<Instant>,
}
pub(crate) fn names(snapshot: &Value) -> BTreeMap<String, String> {
    let plan = &snapshot["plan"];
    let registry = plan["workspace"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .zip(plan["replacement"].as_str())
        .and_then(|(id, text)| tf_catalog::RegistrySnapshot::parse(id, text).ok());
    registry
        .map(|r| {
            r.datasets()
                .map(|d| {
                    (
                        d.key().dataset_id().to_string(),
                        d.path().as_str().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}
impl<W: Write> Display<W> {
    pub fn new(writer: W, options: Options, snapshot: &Value) -> io::Result<Self> {
        let labels = names(snapshot);
        let now = Instant::now();
        let mut jobs: Vec<_> = snapshot["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|j| Job {
                id: j["id"].as_str().unwrap_or("").into(),
                name: labels
                    .get(j["dataset"].as_str().unwrap_or(""))
                    .cloned()
                    .unwrap_or_else(|| j["dataset"].as_str().unwrap_or("Unknown dataset").into()),
                state: j["state"].as_str().unwrap_or("PLANNED").into(),
                since: now,
                elapsed: Duration::ZERO,
            })
            .collect();
        let order: BTreeMap<_, _> = snapshot["plan"]["writes"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(i, w)| w["job"].as_str().map(|id| (id, i)))
            .collect();
        jobs.sort_by_key(|j| order.get(j.id.as_str()).copied().unwrap_or(usize::MAX));
        let mut display = Self {
            writer,
            options,
            build: snapshot["id"].as_str().unwrap_or("").into(),
            jobs,
            started: now,
            drawn: 0,
            width: 0,
            frame: 0,
            last_frame: None,
        };
        if display.options.enabled && display.options.live {
            display.draw()?;
        }
        Ok(display)
    }
    pub fn update(&mut self, event: &Value) -> io::Result<()> {
        if event["jobs"].is_array() {
            for job in event["jobs"].as_array().into_iter().flatten() {
                self.state(
                    job["id"].as_str().unwrap_or(""),
                    job["state"].as_str().unwrap_or(""),
                )?;
            }
        } else {
            self.state(
                event["job"].as_str().unwrap_or(""),
                event["state"].as_str().unwrap_or(""),
            )?;
        }
        Ok(())
    }
    fn state(&mut self, id: &str, state: &str) -> io::Result<()> {
        let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) else {
            return Ok(());
        };
        if job.state == state || terminal_state(&job.state) {
            return Ok(());
        }
        if active_state(state) && !active_state(&job.state) {
            job.since = Instant::now();
        }
        if terminal_state(state) {
            job.elapsed = job.since.elapsed();
        }
        job.state = state.into();
        if self.options.enabled && !self.options.live {
            writeln!(
                self.writer,
                "{}  {}  {}",
                self.build,
                terminal::safe(&job.name),
                state
            )?;
            self.writer.flush()?;
        }
        Ok(())
    }
    pub fn draw(&mut self) -> io::Result<()> {
        if !self.options.enabled || !self.options.live {
            return Ok(());
        }
        if self
            .last_frame
            .is_some_and(|last| last.elapsed() < Duration::from_millis(100))
        {
            return Ok(());
        }
        let (width, height) = terminal::dimensions(true);
        self.draw_at(width, height)
    }
    fn draw_at(&mut self, width: usize, height: usize) -> io::Result<()> {
        let mut frame = Vec::new();
        let width = width.saturating_sub(1).max(1);
        // On resize, leave the old frame in scrollback instead of guessing how the terminal reflowed it.
        if self.width != 0 && self.width != width {
            self.drawn = 0;
        }
        self.width = width;
        self.clear(&mut frame)?;
        let style = self.options.style;
        let done = self
            .jobs
            .iter()
            .filter(|j| terminal_state(&j.state))
            .count();
        let count = self.jobs.len();
        let bar_width = 18;
        let fill = (done * bar_width).checked_div(count).unwrap_or(0);
        let bar = format!("[{}{}]", "=".repeat(fill), " ".repeat(bar_width - fill));
        let heading = if width < 60 {
            format!("Build {bar} {done}/{count} complete")
        } else {
            format!(
                "Build {}   {bar} {done}/{count} complete   {:.1}s",
                clip(&self.build, 11),
                self.started.elapsed().as_secs_f64()
            )
        };
        let title = clip(&heading, width);
        writeln!(frame, "\x1b[?25l{}", style.paint(&title, "1;36"))?;
        self.drawn = 1;
        let capacity = height.saturating_sub(4);
        let mut visible: Vec<_> = self.jobs.iter().collect();
        if visible.len() > capacity {
            visible.sort_by_key(|j| {
                if active_state(&j.state) {
                    0
                } else if !terminal_state(&j.state) {
                    1
                } else {
                    2
                }
            });
        }
        for job in visible.into_iter().take(capacity) {
            let active = active_state(&job.state);
            let spinner = if style.unicode {
                ["◐", "◓", "◑", "◒"][self.frame % 4]
            } else {
                ["|", "/", "-", "\\"][self.frame % 4]
            };
            let name_width = width.saturating_sub(42).clamp(1, 42);
            let name = clip(&job.name, name_width);
            let padding = " ".repeat(name_width.saturating_sub(terminal::cells(&name)));
            let elapsed = if terminal_state(&job.state) {
                job.elapsed
            } else {
                job.since.elapsed()
            };
            let line = if active {
                let mut bar = [' '; 10];
                for i in 0..3 {
                    bar[(self.frame + i) % 10] = '=';
                }
                let plain_status = Style::default().status(&job.state);
                let phase = plain_status.get(2..).unwrap_or(&plain_status);
                let text = clip(
                    &format!(
                        "{spinner} {name}{padding}  [{}] {phase}  {:.1}s",
                        bar.iter().collect::<String>(),
                        elapsed.as_secs_f64()
                    ),
                    width,
                );
                style.paint(
                    &text,
                    if job.state == "RETRY_WAIT" {
                        "33"
                    } else {
                        "36"
                    },
                )
            } else {
                // Clip before adding colour sequences so escape bytes never count as columns.
                let plain = Style {
                    color: false,
                    ..style
                }
                .status(&job.state);
                let (symbol, label) = plain.split_once(' ').unwrap_or(("?", &plain));
                let text = clip(
                    &format!(
                        "{symbol} {name}{padding}  {}{label}{}",
                        if terminal_state(&job.state) {
                            "[==========] "
                        } else {
                            "             "
                        },
                        if terminal_state(&job.state) {
                            format!("  {:.1}s", elapsed.as_secs_f64())
                        } else {
                            String::new()
                        }
                    ),
                    width,
                );
                let code = if matches!(job.state.as_str(), "SUCCEEDED" | "CACHED") {
                    "32"
                } else if matches!(job.state.as_str(), "FAILED" | "BLOCKED") {
                    "31"
                } else if terminal_state(&job.state) {
                    "33"
                } else {
                    "2"
                };
                style.paint(&text, code)
            };
            writeln!(frame, "{line}")?;
            self.drawn += 1;
        }
        if count > capacity && height >= 3 {
            writeln!(
                frame,
                "{}",
                clip(
                    &format!(
                        "  {} more jobs; active jobs shown first. Full results follow.",
                        count - capacity
                    ),
                    width
                )
            )?;
            self.drawn += 1;
        }
        self.frame += 1;
        self.last_frame = Some(Instant::now());
        self.writer.write_all(&frame)?;
        self.writer.flush()
    }
    fn clear(&mut self, frame: &mut Vec<u8>) -> io::Result<()> {
        if self.drawn > 0 {
            write!(frame, "\r\x1b[{}A", self.drawn)?;
            for _ in 0..self.drawn {
                writeln!(frame, "\x1b[2K")?;
            }
            write!(frame, "\x1b[{}A", self.drawn)?;
            self.drawn = 0;
        }
        Ok(())
    }
    pub fn finish(&mut self) -> io::Result<()> {
        if self.options.live {
            self.last_frame = None;
            self.draw()?;
            write!(self.writer, "\x1b[0m\x1b[?25h")?;
            self.writer.flush()?;
            self.drawn = 0;
        }
        Ok(())
    }
}
impl<W: Write> Drop for Display<W> {
    fn drop(&mut self) {
        if self.options.live {
            let _ = write!(self.writer, "\x1b[0m\x1b[?25h");
            let _ = self.writer.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot() -> Value {
        json!({"id":"12345678-1234-1234-1234-123456789abc", "jobs":[
            {"id":"a", "dataset":"raw/orders", "state":"PLANNED"},
            {"id":"b", "dataset":"curated/summary", "state":"PLANNED"}
        ], "plan":{"writes":[{"job":"a"},{"job":"b"}]}})
    }
    #[test]
    fn final_snapshot_recovers_dropped_events_and_terminal_rows_stay_terminal() -> io::Result<()> {
        let mut output = Vec::new();
        {
            let mut display = Display::new(
                &mut output,
                Options {
                    enabled: true,
                    ..Default::default()
                },
                &snapshot(),
            )?;
            display.update(&json!({"job":"a","state":"RUNNING"}))?;
            display.update(
                &json!({"jobs":[{"id":"a","state":"SUCCEEDED"},{"id":"b","state":"CACHED"}]}),
            )?;
            display.update(&json!({"job":"a","state":"RUNNING"}))?;
            display.update(&json!({"job":"unknown","state":"FAILED"}))?;
            assert_eq!(display.jobs[0].state, "SUCCEEDED");
            assert_eq!(display.jobs[1].state, "CACHED");
        }
        let text = String::from_utf8_lossy(&output);
        assert_eq!(text.lines().count(), 3);
        assert!(text.contains("raw/orders  SUCCEEDED"));
        assert!(text.contains("curated/summary  CACHED"));
        assert!(!text.contains('\x1b'));
        Ok(())
    }
    #[test]
    fn animation_finishes_in_place_and_restores_cursor_on_drop() -> io::Result<()> {
        let mut output = Vec::new();
        {
            let mut display = Display::new(
                &mut output,
                Options {
                    enabled: true,
                    ..Default::default()
                },
                &snapshot(),
            )?;
            display.options.live = true;
            display.options.style = Style {
                rich: true,
                color: true,
                unicode: true,
            };
            display.update(&json!({"job":"a","state":"RUNNING"}))?;
            display.draw_at(100, 20)?;
            display.update(&json!({"job":"a","state":"RETRY_WAIT"}))?;
            display.draw_at(100, 20)?;
            display.update(&json!({"job":"a","state":"SUCCEEDED"}))?;
            display.update(&json!({"job":"b","state":"FAILED"}))?;
            display.draw_at(100, 20)?;
        }
        let text = String::from_utf8_lossy(&output);
        assert!(text.contains("◐ raw/orders"));
        assert!(text.contains("\x1b[33m◓ raw/orders"));
        assert!(text.contains("Retrying"));
        assert!(text.contains("\x1b[32m✓ raw/orders"));
        assert!(text.contains("\x1b[31m✗ curated/summary"));
        assert!(text.contains("2/2 complete"));
        assert!(text.contains("\x1b[3A"));
        assert!(text.ends_with("\x1b[0m\x1b[?25h"));
        Ok(())
    }
    #[test]
    fn small_terminal_prioritizes_active_jobs_and_never_wraps_its_frame() -> io::Result<()> {
        let mut output = Vec::new();
        let mut data = snapshot();
        data["jobs"][0]["dataset"] = json!("長い名前/\u{1b}[2J malicious");
        {
            let mut display = Display::new(
                &mut output,
                Options {
                    enabled: true,
                    ..Default::default()
                },
                &data,
            )?;
            display.jobs[1].state = "RUNNING".into();
            display.jobs[1].name = "active".into();
            display.options.live = true;
            display.draw_at(50, 5)?;
            assert_eq!(display.drawn, 3);
        }
        let text = String::from_utf8_lossy(&output);
        assert!(text.contains("active"));
        assert!(text.contains("1 more jobs"));
        for line in text.lines() {
            let plain = line
                .replace("\x1b[?25l", "")
                .replace("\x1b[0m", "")
                .replace("\x1b[?25h", "");
            assert!(terminal::cells(&plain) < 50, "{plain}");
        }
        assert!(!text.contains("\x1b[2J"));
        Ok(())
    }
    #[test]
    fn disabled_output_and_failed_writer_do_not_change_job_outcomes() -> io::Result<()> {
        let mut output = Vec::new();
        {
            let mut display = Display::new(&mut output, Options::default(), &snapshot())?;
            display.update(&json!({"job":"a","state":"FAILED"}))?;
            display.draw()?;
            display.finish()?;
            assert_eq!(display.jobs[0].state, "FAILED");
        }
        assert!(output.is_empty());
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut display = Display::new(
            Broken,
            Options {
                enabled: true,
                ..Default::default()
            },
            &snapshot(),
        )?;
        let error = display.update(&json!({"job":"a","state":"FAILED"})).err();
        assert_eq!(error.map(|e| e.kind()), Some(io::ErrorKind::BrokenPipe));
        assert_eq!(display.jobs[0].state, "FAILED");
        Ok(())
    }
}
