//! Structured logs: one JSON object per line on standard output.
//!
//! Every line has `ts`, `level`, `service`, `event` and, where one exists, `correlation`. The
//! same correlation id appears in the browser worker, the BFF, the backend, the kernel's events
//! and the notice the browser receives, so one request can be followed end to end with a text
//! search. Callers pass only what is safe to log; passwords, tokens and record contents are
//! never given to this module.

use serde_json::{json, Map, Value as Json};
use std::io::Write;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Write one finished log line (no trailing newline in `line`).
///
/// Every structured log in Strata goes through here, the runtime's telemetry included. The line
/// is written with `print!`, so a service writes the same bytes to standard output as before, while
/// a test binary captures it the way libtest captures any printed output: it is shown only when
/// the test fails. Setting `STRATA_TEST_LOG` writes lines straight to standard output instead, past
/// the capture, so a developer can read a test's logs. The variable is read once per process.
///
/// Known difference: `print!` panics if standard output is closed, where the previous direct write
/// silently dropped the line.
pub fn emit_line(line: &str) {
    static DIRECT: OnceLock<bool> = OnceLock::new();
    if *DIRECT.get_or_init(|| std::env::var_os("STRATA_TEST_LOG").is_some()) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
    } else {
        print!("{line}\n");
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
