//! Reading a budget requirement's statement into a threshold.
//!
//! A budget statement states a measurable target in English. Three forms are read, tried in
//! this order, and the first that matches wins:
//!
//! 1. `within <N> <time unit>`, optionally followed by `at the <P>th percentile`: a latency
//!    budget. Without a percentile the bound is on every observation (quantile 1).
//! 2. `at least|at most|no more than|fewer than <N> [percent|<time unit>]`: a bound on a
//!    ratio (percent), a duration (time unit) or a plain count.
//!
//! Anything else (a number written as a word, a target with no number, a clause such as "or 15
//! minutes after it was shown") is [`Budget::Unparsed`], and the monitoring backend keeps the
//! statement as an annotation with no threshold. Time units are converted to seconds.

use std::sync::OnceLock;

use regex::Regex;

/// When an alert on the measured value fires, relative to the threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Violation {
    /// The value is above the threshold.
    Above,
    /// The value is at or above the threshold.
    AtOrAbove,
    /// The value is below the threshold.
    Below,
}

impl Violation {
    /// The PromQL comparison operator.
    pub fn operator(self) -> &'static str {
        match self {
            Self::Above => ">",
            Self::AtOrAbove => ">=",
            Self::Below => "<",
        }
    }
}

/// What a budget statement asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Budget {
    /// A latency quantile must stay within a number of seconds.
    Latency {
        /// The quantile in `(0, 1]`; 1 when the statement names no percentile.
        quantile: f64,
        /// The limit in seconds.
        seconds: f64,
    },
    /// A ratio (percent in the statement, a fraction here).
    Ratio {
        /// When it is violated.
        violation: Violation,
        /// The threshold as a fraction of 1.
        fraction: f64,
    },
    /// A duration bound, in seconds.
    Duration {
        /// When it is violated.
        violation: Violation,
        /// The threshold in seconds.
        seconds: f64,
    },
    /// A bound on a count or size.
    Count {
        /// When it is violated.
        violation: Violation,
        /// The threshold.
        value: f64,
    },
    /// The statement's target could not be read.
    Unparsed,
}

fn within_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\bwithin (\d+(?:\.\d+)?) (milliseconds?|ms|seconds?|minutes?|hours?|days?)\b(?:[^.]*?\bat the (\d+(?:\.\d+)?)(?:st|nd|rd|th) percentile)?",
        )
        .expect("the within pattern is valid")
    })
}

fn bound_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(at least|at most|no more than|fewer than) (\d+(?:\.\d+)?)(?: (percent|milliseconds?|ms|seconds?|minutes?|hours?|days?))?\b",
        )
        .expect("the bound pattern is valid")
    })
}

/// Seconds in one of the time units the patterns accept, or `None` for another word.
fn seconds_per(unit: &str) -> Option<f64> {
    match unit.to_ascii_lowercase().as_str() {
        "ms" | "millisecond" | "milliseconds" => Some(0.001),
        "second" | "seconds" => Some(1.0),
        "minute" | "minutes" => Some(60.0),
        "hour" | "hours" => Some(3600.0),
        "day" | "days" => Some(86400.0),
        _ => None,
    }
}

/// Reads a budget statement.
pub fn parse(statement: &str) -> Budget {
    if let Some(c) = within_re().captures(statement) {
        let number: f64 = c[1].parse().unwrap_or(f64::NAN);
        let per = seconds_per(&c[2]);
        let quantile = match c.get(3) {
            Some(p) => p.as_str().parse::<f64>().map(|p| p / 100.0).ok(),
            None => Some(1.0),
        };
        if let (Some(per), Some(quantile)) = (per, quantile) {
            if number.is_finite() && quantile > 0.0 && quantile <= 1.0 {
                return Budget::Latency {
                    quantile,
                    seconds: number * per,
                };
            }
        }
    }
    if let Some(c) = bound_re().captures(statement) {
        let number: f64 = c[2].parse().unwrap_or(f64::NAN);
        if !number.is_finite() {
            return Budget::Unparsed;
        }
        let violation = match c[1].to_ascii_lowercase().as_str() {
            "at least" => Violation::Below,
            "fewer than" => Violation::AtOrAbove,
            _ => Violation::Above,
        };
        return match c.get(3).map(|m| m.as_str().to_ascii_lowercase()) {
            Some(unit) if unit == "percent" => Budget::Ratio {
                violation,
                fraction: number / 100.0,
            },
            Some(unit) => match seconds_per(&unit) {
                Some(per) => Budget::Duration {
                    violation,
                    seconds: number * per,
                },
                None => Budget::Unparsed,
            },
            None => Budget::Count {
                violation,
                value: number,
            },
        };
    }
    Budget::Unparsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_latency_with_a_percentile() {
        assert_eq!(
            parse("Provisioning SHALL complete within 30 seconds at the 95th percentile."),
            Budget::Latency {
                quantile: 0.95,
                seconds: 30.0
            }
        );
        assert_eq!(
            parse("A session lookup within 20 ms at the 99th percentile."),
            Budget::Latency {
                quantile: 0.99,
                seconds: 0.02
            }
        );
    }

    #[test]
    fn a_limit_without_a_percentile_bounds_every_observation() {
        assert_eq!(
            parse("Honoured by every instance within 5 seconds."),
            Budget::Latency {
                quantile: 1.0,
                seconds: 5.0
            }
        );
        assert_eq!(
            parse("The platform SHALL serve again within 15 minutes."),
            Budget::Latency {
                quantile: 1.0,
                seconds: 900.0
            }
        );
    }

    #[test]
    fn bounds_on_ratios_durations_and_counts() {
        assert_eq!(
            parse("At least 99.95 percent of requests SHALL be answered."),
            Budget::Ratio {
                violation: Violation::Below,
                fraction: 0.9995
            }
        );
        assert_eq!(
            parse("A token SHALL be valid for at most 5 minutes."),
            Budget::Duration {
                violation: Violation::Above,
                seconds: 300.0
            }
        );
        assert_eq!(
            parse("Fewer than 500000 metric series at the reference load."),
            Budget::Count {
                violation: Violation::AtOrAbove,
                value: 500_000.0
            }
        );
    }

    #[test]
    fn statements_without_a_readable_number_are_unparsed() {
        assert_eq!(
            parse("No more than one message per 10 seconds."),
            Budget::Unparsed
        );
        assert_eq!(parse("A preview SHALL stop being valid."), Budget::Unparsed);
    }
}
