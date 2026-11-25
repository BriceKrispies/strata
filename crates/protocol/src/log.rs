//! Structured logs: one JSON object per line on standard output.
//!
//! Every line has `ts`, `level`, `service`, `event` and, where one exists, `correlation`. The
//! same correlation id appears in the browser worker, the BFF, the backend, the kernel's events
//! and the notice the browser receives, so one request can be followed end to end with a text
//! search. Callers pass only what is safe to log; passwords, tokens and record contents are
//! never given to this module.

use serde_json::{json, Map, Value as Json};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Set by a service binary's `main` through [`use_stdout`].
static STDOUT_SERVICE: AtomicBool = AtomicBool::new(false);

/// Switch this process's log lines to direct writes on standard output, for a service binary.
///
/// Call it first thing in `main`. Writes ignore errors, so a closed standard output drops lines
/// rather than panicking. Without this call, lines go through `print!`, which libtest captures
/// inside a test binary; a test binary that needs its logs sets `STRATA_TEST_LOG`.
pub fn use_stdout() {
    STDOUT_SERVICE.store(true, Ordering::Relaxed);
}

/// Write one finished log line (no trailing newline in `line`).
///
/// Every structured log in Strata goes through here, the runtime's telemetry included. The sink
/// is chosen per process:
/// - a service binary that called [`use_stdout`] writes directly to standard output, ignoring
///   errors, exactly as the services always have;
/// - otherwise the line goes through `print!`, so a test binary captures it the way libtest
///   captures any printed output (shown only when the test fails), and
/// - `STRATA_TEST_LOG` set in the environment writes directly to standard output past that
///   capture, so a developer can read a test's logs. It is read once per process.
pub fn emit_line(line: &str) {
    static TEST_LOG: OnceLock<bool> = OnceLock::new();
    let direct = STDOUT_SERVICE.load(Ordering::Relaxed)
        || *TEST_LOG.get_or_init(|| std::env::var_os("STRATA_TEST_LOG").is_some());
    if direct {
        write_stdout_line(&mut std::io::stdout().lock(), line);
    } else {
        println!("{line}");
    }
}

/// The direct sink's write: one line, errors ignored.
fn write_stdout_line<W: Write>(out: &mut W, line: &str) {
    let _ = writeln!(out, "{line}");
}

#[cfg(test)]
mod tests {
    use super::write_stdout_line;
    use std::io::{self, Write};

    /// Standard output that has been closed: every write fails.
    struct Closed;

    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "stdout is closed",
            ))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "stdout is closed",
            ))
        }
    }

    #[test]
    fn the_stdout_sink_drops_a_line_it_cannot_write_without_panicking() {
        write_stdout_line(&mut Closed, "{\"event\":\"x\"}");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A logger bound to one service name.
#[derive(Clone, Copy, Debug)]
pub struct Logger {
    pub service: &'static str,
}

impl Logger {
    /// Write one line. `fields` must be a JSON object; its members are merged into the line.
    pub fn log(&self, level: Level, event: &str, correlation: &str, fields: Json) {
        let mut line = Map::new();
        line.insert("ts".into(), json!(now_ms()));
        line.insert("level".into(), json!(level.name()));
        line.insert("service".into(), json!(self.service));
        line.insert("event".into(), json!(event));
        if !correlation.is_empty() {
            line.insert("correlation".into(), json!(correlation));
        }
        if let Json::Object(extra) = fields {
            line.extend(extra);
        }
        emit_line(&Json::Object(line).to_string());
    }

    pub fn info(&self, event: &str, correlation: &str, fields: Json) {
        self.log(Level::Info, event, correlation, fields);
    }

    pub fn warn(&self, event: &str, correlation: &str, fields: Json) {
        self.log(Level::Warn, event, correlation, fields);
    }

    pub fn error(&self, event: &str, correlation: &str, fields: Json) {
        self.log(Level::Error, event, correlation, fields);
    }
}
