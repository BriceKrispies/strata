//! The decisions of the dispatcher about one message, as pure functions over what the store
//! holds: whether the suppression list stops it (D-6562, EML-1562), whether the allowance of its
//! class holds it (D-6563, EML-1563), and whether the complaint breaker holds it (D-6564,
//! EML-1564).

use super::store::{BreakerState, Suppression, Usage, ALL, ALL_PROMOTIONAL};
use super::{AllowanceClass, Purpose};

/// Whether `entries` stop a message of `category` (whose purpose is `purpose`) to their
/// address. A suppression of the whole address stops every message, sign-in mail included; one
/// of every promotional category stops a message of a promotional category only; one of a
/// category stops a message of that category. Sign-in mail and mail with no category pass
/// `None` for both and are stopped by the whole address only.
pub fn suppressed(
    entries: &[Suppression],
    category: Option<&str>,
    purpose: Option<Purpose>,
) -> bool {
    entries.iter().any(|entry| {
        entry.category == ALL
            || (entry.category == ALL_PROMOTIONAL && purpose == Some(Purpose::Promotional))
            || category.is_some_and(|c| entry.category == c)
    })
}

/// The most bulk messages a month may send: eighty percent of the monthly allowance (D-6563).
pub fn bulk_cap(limit: u32) -> u32 {
    u32::try_from(u64::from(limit) * 80 / 100).unwrap_or(u32::MAX)
}

/// Whether a message of `class` is held by the allowance, given what the month has used: sign-in
/// mail never; transactional mail when the whole allowance is used; bulk mail when bulk mail has
/// used eighty percent of it, or the whole allowance is used (D-6563, EML-1563).
pub fn allowance_holds(class: AllowanceClass, usage: Usage, limit: u32) -> bool {
    let total = usage.transactional.saturating_add(usage.bulk);
    match class {
        AllowanceClass::SignIn => false,
        AllowanceClass::Transactional => total >= limit,
        AllowanceClass::Bulk => usage.bulk >= bulk_cap(limit) || total >= limit,
    }
}

/// What the breaker measures over the last seven days of the log (D-6564).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rates {
    /// Messages the provider accepted.
    pub accepted: u64,
    pub complaints: u64,
    /// Permanent bounces.
    pub bounces: u64,
}

/// The fewest accepted messages the rates are measured over (D-6564).
pub const BREAKER_MINIMUM: u64 = 1_000;

/// The breaker after a measurement (D-6564, EML-1564): bulk mail is held at 0.5 percent
/// complaints or 10 percent permanent bounces, transactional mail at 1 percent complaints, each
/// over at least one thousand accepted messages; a hold ends when both rates are below their
/// warning levels of 0.1 and 5 percent. Under the minimum nothing is measured and nothing held.
pub fn breaker(rates: Rates, previous: BreakerState, now: i64) -> BreakerState {
    if rates.accepted < BREAKER_MINIMUM {
        return BreakerState {
            bulk_held: false,
            transactional_held: false,
            measured_at: now,
        };
    }
    // In whole numbers: complaints / accepted >= 5 / 1000 is complaints * 1000 >= accepted * 5.
    let at_least = |count: u64, per_thousand: u64| count * 1_000 >= rates.accepted * per_thousand;
    let bulk_trips = at_least(rates.complaints, 5) || at_least(rates.bounces, 100);
    let transactional_trips = at_least(rates.complaints, 10);
    let calm = !at_least(rates.complaints, 1) && !at_least(rates.bounces, 50);
    BreakerState {
        bulk_held: bulk_trips || (previous.bulk_held && !calm),
        transactional_held: transactional_trips || (previous.transactional_held && !calm),
        measured_at: now,
    }
}

/// Whether the breaker holds a message of `class`; sign-in mail is never held.
pub fn breaker_holds(state: &BreakerState, class: AllowanceClass) -> bool {
    match class {
        AllowanceClass::SignIn => false,
        AllowanceClass::Transactional => state.transactional_held,
        AllowanceClass::Bulk => state.bulk_held || state.transactional_held,
    }
}

/// The civil date of a day counted from 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month as u32)
}

/// The day from 1970-01-01 of the first of a month.
fn days_from_civil(year: i64, month: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The allowance period of a time: its month in UTC, as `YYYY-MM`.
pub fn period(at_ms: i64) -> String {
    let (year, month) = civil(at_ms.div_euclid(86_400_000));
    format!("{year:04}-{month:02}")
}

/// The first millisecond of the month after the month of `at_ms`, in UTC: when a message held by
/// the allowance is attempted again.
pub fn next_period_start(at_ms: i64) -> i64 {
    let (year, month) = civil(at_ms.div_euclid(86_400_000));
    let (year, month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    days_from_civil(year, month) * 86_400_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::store::SuppressionReason;

    fn entry(category: &str) -> Suppression {
        Suppression {
            address_hash: "h".into(),
            category: category.into(),
            reason: SuppressionReason::Unsubscribe,
            recorded_at: 0,
        }
    }

    // Verifies: EML-1562
    #[test]
    fn a_promotional_unsubscribe_stops_promotional_mail_only() {
        let all_promotional = [entry(ALL_PROMOTIONAL)];
        assert!(suppressed(
            &all_promotional,
            Some("news"),
            Some(Purpose::Promotional)
        ));
        assert!(!suppressed(
            &all_promotional,
            Some("receipts"),
            Some(Purpose::Transactional)
        ));
        assert!(!suppressed(&all_promotional, None, None));
        let one = [entry("news")];
        assert!(suppressed(&one, Some("news"), Some(Purpose::Promotional)));
        assert!(!suppressed(
            &one,
            Some("offers"),
            Some(Purpose::Promotional)
        ));
        assert!(suppressed(&[entry(ALL)], None, None));
    }

    // Verifies: EML-1563
    #[test]
    fn bulk_stops_at_eighty_percent_and_transactional_at_the_whole() {
        let usage = Usage {
            transactional: 0,
            bulk: 8,
        };
        assert!(allowance_holds(AllowanceClass::Bulk, usage, 10));
        assert!(!allowance_holds(AllowanceClass::Transactional, usage, 10));
        let full = Usage {
            transactional: 2,
            bulk: 8,
        };
        assert!(allowance_holds(AllowanceClass::Transactional, full, 10));
        assert!(!allowance_holds(AllowanceClass::SignIn, full, 10));
    }

    // Verifies: EML-1564
    #[test]
    fn the_breaker_trips_at_its_thresholds_and_ends_below_the_warning() {
        let open = breaker(
            Rates {
                accepted: 1_000,
                complaints: 5,
                bounces: 0,
            },
            BreakerState::default(),
            1,
        );
        assert!(open.bulk_held && !open.transactional_held);
        // Between warning and hold the hold stays.
        let still = breaker(
            Rates {
                accepted: 1_000,
                complaints: 2,
                bounces: 0,
            },
            open,
            2,
        );
        assert!(still.bulk_held);
        let ended = breaker(
            Rates {
                accepted: 1_000,
                complaints: 0,
                bounces: 10,
            },
            still,
            3,
        );
        assert!(!ended.bulk_held);
        let few = breaker(
            Rates {
                accepted: 999,
                complaints: 999,
                bounces: 0,
            },
            BreakerState::default(),
            4,
        );
        assert!(!few.bulk_held && !few.transactional_held);
    }

    #[test]
    fn periods_are_utc_months() {
        // 2026-11-20T00:00:00Z
        let at = 1_795_132_800_000;
        assert_eq!(period(at), "2026-11");
        assert_eq!(period(next_period_start(at)), "2026-12");
        assert_eq!(next_period_start(at), 1_796_083_200_000);
    }
}
