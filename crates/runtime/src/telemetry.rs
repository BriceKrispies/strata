//! Telemetry hooks: events, metrics and finished spans.
//!
//! The runtime reports what it does through [`Telemetry`] and nothing else. Two implementations
//! ship: [`NoTelemetry`], which drops everything, and [`JsonLines`], which writes one JSON object
//! per line on standard output in the same shape as the services' structured logs (`ts`, `level`,
//! `service`, `event`, then the fields). Neither pulls in a tracing framework; a deployment that
//! wants one implements the trait.
//!
//! Callers pass only what is safe to record. The runtime itself reports names, keys, receivers,
//! counts and durations, never event payloads or record contents.

use serde_json::{json, Map, Value as Json};
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How much a reported event matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Detail for whoever is debugging.
    Debug,
    /// Normal operation worth recording.
    Info,
    /// Something went wrong and was handled, such as a delivery that will be retried.
    Warn,
    /// Something went wrong and was not handled.
    Error,
}

impl Level {
    /// The level's lower-case name.
    pub fn name(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

/// The value of one reported field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value<'a> {
    /// Text.
    Str(&'a str),
    /// A signed integer.
    Int(i64),
    /// An unsigned integer.
    Uint(u64),
    /// A floating-point number.
    Float(f64),
    /// A flag.
    Bool(bool),
}

impl<'a> From<&'a str> for Value<'a> {
    fn from(value: &'a str) -> Self {
        Value::Str(value)
    }
}

impl<'a> From<&'a String> for Value<'a> {
    fn from(value: &'a String) -> Self {
        Value::Str(value)
    }
}

impl From<i64> for Value<'_> {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<u64> for Value<'_> {
    fn from(value: u64) -> Self {
        Value::Uint(value)
    }
}

impl From<usize> for Value<'_> {
    fn from(value: usize) -> Self {
        Value::Uint(value as u64)
    }
}

impl From<u32> for Value<'_> {
    fn from(value: u32) -> Self {
        Value::Uint(u64::from(value))
    }
}

impl From<f64> for Value<'_> {
    fn from(value: f64) -> Self {
        Value::Float(value)
    }
}

impl From<bool> for Value<'_> {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl Value<'_> {
    fn to_json(self) -> Json {
        match self {
            Value::Str(text) => json!(text),
            Value::Int(n) => json!(n),
            Value::Uint(n) => json!(n),
            Value::Float(n) => json!(n),
            Value::Bool(b) => json!(b),
        }
    }
}

/// One named field of a report.
pub type Field<'a> = (&'a str, Value<'a>);

/// Where the runtime reports what it does.
pub trait Telemetry: Send + Sync + 'static {
    /// Something happened.
    fn event(&self, level: Level, name: &str, fields: &[Field<'_>]);

    /// A measurement: a count, a size or a duration in milliseconds.
    fn metric(&self, name: &str, value: f64, fields: &[Field<'_>]);

    /// A unit of work finished after `elapsed`, successfully when `ok`.
    fn span(&self, name: &str, elapsed: Duration, ok: bool, fields: &[Field<'_>]);
}

/// Telemetry that records nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoTelemetry;

impl Telemetry for NoTelemetry {
    fn event(&self, _: Level, _: &str, _: &[Field<'_>]) {}
    fn metric(&self, _: &str, _: f64, _: &[Field<'_>]) {}
    fn span(&self, _: &str, _: Duration, _: bool, _: &[Field<'_>]) {}
}

/// Telemetry written as JSON lines on standard output.
#[derive(Clone, Copy, Debug)]
pub struct JsonLines {
    /// The service name put on every line.
    pub service: &'static str,
}

impl JsonLines {
    fn write(&self, level: Level, name: &str, fields: &[Field<'_>], extra: &[(&str, Json)]) {
        let mut line = Map::new();
        line.insert(
            "ts".into(),
            json!(crate::clock::Clock::now(&crate::SystemClock).0),
        );
        line.insert("level".into(), json!(level.name()));
        line.insert("service".into(), json!(self.service));
        line.insert("event".into(), json!(name));
        for (key, value) in extra {
            line.insert((*key).to_string(), value.clone());
        }
        for (key, value) in fields {
            line.insert((*key).to_string(), value.to_json());
        }
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{}", Json::Object(line));
    }
}

impl Telemetry for JsonLines {
    fn event(&self, level: Level, name: &str, fields: &[Field<'_>]) {
        self.write(level, name, fields, &[]);
    }

    fn metric(&self, name: &str, value: f64, fields: &[Field<'_>]) {
        self.write(Level::Debug, name, fields, &[("value", json!(value))]);
    }

    fn span(&self, name: &str, elapsed: Duration, ok: bool, fields: &[Field<'_>]) {
        let level = if ok { Level::Info } else { Level::Warn };
        self.write(
            level,
            name,
            fields,
            &[("ms", json!(elapsed.as_millis() as u64)), ("ok", json!(ok))],
        );
    }
}

/// A span being timed. Report it with [`Span::finish`]; dropping it unfinished reports nothing.
pub struct Span {
    telemetry: Arc<dyn Telemetry>,
    name: &'static str,
    started: Instant,
}

impl Span {
    /// Start timing a unit of work called `name`.
    pub fn start(telemetry: &Arc<dyn Telemetry>, name: &'static str) -> Span {
        Span {
            telemetry: telemetry.clone(),
            name,
            started: Instant::now(),
        }
    }

    /// Report the span as finished, successfully when `ok`.
    pub fn finish(self, ok: bool, fields: &[Field<'_>]) {
        self.telemetry
            .span(self.name, self.started.elapsed(), ok, fields);
    }
}
