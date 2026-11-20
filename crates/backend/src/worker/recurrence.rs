//! Recurrence rules and their time zones (SCH-1573, SCH-1574, D-6567).
//!
//! A recurrence is the RRULE value of RFC 5545 section 3.3.10 limited to the subset SCH-1574
//! names, with the added part `TZID`:
//!
//! - `FREQ` of `HOURLY`, `DAILY`, `WEEKLY` or `MONTHLY`;
//! - `INTERVAL`, `BYDAY` without an ordinal, `BYMONTHDAY`, one `BYHOUR`, one `BYMINUTE`, `COUNT`
//!   and `UNTIL`;
//! - `TZID`, a name of the IANA time zone database, in which the `BY` parts are read; the tenant's
//!   zone when it is absent.
//!
//! Any other part, a repeated part, a list of hours or minutes, an ordinal before a weekday, a
//! `COUNT` together with an `UNTIL`, `BYMONTHDAY` on a weekly rule and an unknown zone are refused
//! with a message ([`Recurrence::parse`]); the route answers them as `invalid`.
//!
//! **Reading a wall-clock time** is RFC 5545 section 3.3.5 and SCH-1573: a local time that does not
//! exist because clocks go forward is read at the offset that held before the change, and a local
//! time that occurs twice is the first of the two. Both are `jiff`'s `compatible` reading of an
//! ambiguous timestamp, which is the one used here. `HOURLY` steps by elapsed hours and the
//! other frequencies by wall-clock days, so a daily rule fires once on the day clocks change, and
//! a month without the named day is skipped.
//!
//! **Where the rule counts from** is DTSTART, which a rule does not carry: the schedule keeps the
//! time it was defined (or its rule last changed) as its anchor, and the wall-clock time of the
//! anchor, to the minute, stands in for any `BYHOUR` or `BYMINUTE` the rule leaves out. An
//! occurrence before the anchor is no occurrence, and `COUNT` counts from the anchor.
//!
//! **Zone data.** The time zone rules are `jiff`'s, with its bundled IANA database
//! (`tzdb-bundle-always`), so that every platform reads the same version, the one the release
//! carries (D-6567). No zone rule is written here.

use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use jiff::Timestamp as Instant;

const HOUR_MS: i64 = 3_600_000;
/// Periods of a rule that may pass without an occurrence before the rule is taken to have none
/// left (a month with no 31st for ever, an interval of years).
const MAX_EMPTY_PERIODS: u32 = 100_000;

/// The frequencies of the subset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Freq {
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

/// The end of a rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Until {
    /// A UTC date-time: occurrences up to and including this instant.
    Instant(i64),
    /// A date-time in the zone of the rule, as the day number and the second of the day.
    Local { day: i64, second: i64 },
    /// A date: occurrences on or before that day of the zone.
    Date(i64),
}

/// A recurrence rule with its time zone.
#[derive(Clone, Debug)]
pub struct Recurrence {
    freq: Freq,
    interval: i64,
    /// Weekdays, 0 for Monday to 6 for Sunday, ascending.
    by_day: Vec<i64>,
    /// Days of the month, 1 to 31 or -31 to -1 from the end.
    by_month_day: Vec<i64>,
    by_hour: Option<i64>,
    by_minute: Option<i64>,
    count: Option<u64>,
    until: Option<Until>,
    zone: TimeZone,
    zone_name: String,
}

/// The milliseconds since the Unix epoch of a UTC date and time, for the callers that state a
/// time by its parts.
pub fn utc_ms(year: i64, month: i64, day: i64, hour: i64, minute: i64) -> i64 {
    days_from_civil(year, month, day) * 86_400_000 + (hour * 60 + minute) * 60_000
}

/// The day number (days since 1970-01-01) of a civil date (proleptic Gregorian).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day number: `(year, month, day)`.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { y + 1 } else { y }, month, day)
}

/// 0 for Monday to 6 for Sunday (1970-01-01 was a Thursday).
fn weekday(day: i64) -> i64 {
    (day + 3).rem_euclid(7)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        _ => 28,
    }
}

fn month_index(day: i64) -> i64 {
    let (year, month, _) = civil_from_days(day);
    year * 12 + month - 1
}

/// A whole number written in digits only, within `low..=high`.
fn number(text: &str, part: &str, low: i64, high: i64) -> Result<i64, String> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{part}: {text} is not a whole number"));
    }
    let value: i64 = text
        .parse()
        .map_err(|_| format!("{part}: {text} is out of range"))?;
    if (low..=high).contains(&value) {
        Ok(value)
    } else {
        Err(format!("{part}: {value} is not between {low} and {high}"))
    }
}

/// The weekday number of `MO` to `SU`; a name with an ordinal is not in the subset.
fn weekday_of(name: &str) -> Result<i64, String> {
    match name.to_ascii_uppercase().as_str() {
        "MO" => Ok(0),
        "TU" => Ok(1),
        "WE" => Ok(2),
        "TH" => Ok(3),
        "FR" => Ok(4),
        "SA" => Ok(5),
        "SU" => Ok(6),
        _ => Err(format!(
            "BYDAY: {name} is not a weekday without an ordinal (MO, TU, WE, TH, FR, SA, SU)"
        )),
    }
}

/// UNTIL as RFC 5545 writes it: a UTC date-time `19971224T000000Z`, a date-time `19971224T000000`
/// in the zone of the rule, or a date `19971224`.
fn parse_until(text: &str) -> Result<Until, String> {
    let bad = || format!("UNTIL: {text} is not a date or a date-time of RFC 5545");
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let date = |s: &str| -> Result<i64, String> {
        if s.len() != 8 || !digits(s) {
            return Err(bad());
        }
        let (y, m, d): (i64, i64, i64) = (
            s[0..4].parse().map_err(|_| bad())?,
            s[4..6].parse().map_err(|_| bad())?,
            s[6..8].parse().map_err(|_| bad())?,
        );
        if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
            return Err(bad());
        }
        Ok(days_from_civil(y, m, d))
    };
    let time = |s: &str| -> Result<i64, String> {
        if s.len() != 6 || !digits(s) {
            return Err(bad());
        }
        let (h, m, sec): (i64, i64, i64) = (
            s[0..2].parse().map_err(|_| bad())?,
            s[2..4].parse().map_err(|_| bad())?,
            s[4..6].parse().map_err(|_| bad())?,
        );
        if h > 23 || m > 59 || sec > 60 {
            return Err(bad());
        }
        Ok(h * 3_600 + m * 60 + sec.min(59))
    };
    let upper = text.to_ascii_uppercase();
    match upper.split_once('T') {
        None => Ok(Until::Date(date(&upper)?)),
        Some((day, clock)) => {
            let day = date(day)?;
            match clock.strip_suffix('Z') {
                Some(clock) => Ok(Until::Instant(day * 86_400_000 + time(clock)? * 1_000)),
                None => Ok(Until::Local {
                    day,
                    second: time(clock)?,
                }),
            }
        }
    }
}

impl Recurrence {
    /// Read `text` as a recurrence of the subset, in the zone `tenant_zone` when the rule names
    /// none (SCH-1574). An optional `RRULE:` in front is accepted. A rule outside the subset, a
    /// malformed one and an unknown zone are an `Err` with the reason.
    pub fn parse(text: &str, tenant_zone: &str) -> Result<Recurrence, String> {
        let body = text.trim();
        let body = match body.get(..6) {
            Some(head) if head.eq_ignore_ascii_case("RRULE:") => &body[6..],
            _ => body,
        };
        if body.is_empty() {
            return Err("the recurrence is empty".to_string());
        }
        let mut seen: Vec<String> = Vec::new();
        let mut freq = None;
        let mut interval = 1_i64;
        let mut by_day: Vec<i64> = Vec::new();
        let mut by_month_day: Vec<i64> = Vec::new();
        let mut by_hour = None;
        let mut by_minute = None;
        let mut count = None;
        let mut until_text: Option<String> = None;
        let mut tzid: Option<String> = None;
        for part in body.split(';').filter(|part| !part.is_empty()) {
            let (name, value) = part
                .split_once('=')
                .ok_or_else(|| format!("{part} is not NAME=VALUE"))?;
            let name = name.trim().to_ascii_uppercase();
            let value = value.trim();
            if seen.contains(&name) {
                return Err(format!("{name} appears twice"));
            }
            seen.push(name.clone());
            match name.as_str() {
                "FREQ" => {
                    freq = Some(match value.to_ascii_uppercase().as_str() {
                        "HOURLY" => Freq::Hourly,
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        other => {
                            return Err(format!(
                                "FREQ: {other} is outside the subset (HOURLY, DAILY, WEEKLY, MONTHLY)"
                            ))
                        }
                    })
                }
                "INTERVAL" => interval = number(value, "INTERVAL", 1, i64::from(u32::MAX))?,
                "BYDAY" => {
                    for day in value.split(',') {
                        let day = weekday_of(day.trim())?;
                        if !by_day.contains(&day) {
                            by_day.push(day);
                        }
                    }
                    by_day.sort_unstable();
                }
                "BYMONTHDAY" => {
                    for day in value.split(',') {
                        let day = number(day.trim(), "BYMONTHDAY", -31, 31)?;
                        if day == 0 {
                            return Err("BYMONTHDAY: 0 is not a day of the month".to_string());
                        }
                        if !by_month_day.contains(&day) {
                            by_month_day.push(day);
                        }
                    }
                }
                "BYHOUR" => by_hour = Some(number(value, "BYHOUR (one hour)", 0, 23)?),
                "BYMINUTE" => by_minute = Some(number(value, "BYMINUTE (one minute)", 0, 59)?),
                "COUNT" => count = Some(number(value, "COUNT", 1, i64::MAX)? as u64),
                "UNTIL" => until_text = Some(value.to_string()),
                "TZID" => {
                    if value.is_empty() {
                        return Err("TZID is empty".to_string());
                    }
                    tzid = Some(value.to_string());
                }
                other => {
                    return Err(format!(
                        "{other} is outside the subset (FREQ, INTERVAL, BYDAY, BYMONTHDAY, BYHOUR, BYMINUTE, COUNT, UNTIL, TZID)"
                    ))
                }
            }
        }
        let freq = freq.ok_or_else(|| "FREQ is missing".to_string())?;
        if count.is_some() && until_text.is_some() {
            return Err("COUNT and UNTIL may not both be given".to_string());
        }
        if freq == Freq::Weekly && !by_month_day.is_empty() {
            return Err("BYMONTHDAY may not be used with a weekly rule".to_string());
        }
        let zone_name = tzid.unwrap_or_else(|| tenant_zone.to_string());
        let zone = TimeZone::get(&zone_name)
            .map_err(|_| format!("{zone_name} is not a time zone of the IANA database"))?;
        let until = until_text.map(|text| parse_until(&text)).transpose()?;
        Ok(Recurrence {
            freq,
            interval,
            by_day,
            by_month_day,
            by_hour,
            by_minute,
            count,
            until,
            zone,
            zone_name,
        })
    }

    /// The name of the zone the rule is read in.
    pub fn zone_name(&self) -> &str {
        &self.zone_name
    }

    /// The day number and the minute of the day, in the zone, of an instant.
    fn local(&self, ms: i64) -> Option<(i64, i64)> {
        let at = Instant::from_millisecond(ms).ok()?;
        let dt = self.zone.to_datetime(at);
        Some((
            days_from_civil(
                i64::from(dt.year()),
                i64::from(dt.month()),
                i64::from(dt.day()),
            ),
            i64::from(dt.hour()) * 60 + i64::from(dt.minute()),
        ))
    }

    /// The instant of a wall-clock time of the zone, read as SCH-1573 says: a time that does not
    /// exist at the offset before the change, a time that occurs twice at its first occurrence.
    fn instant(&self, day: i64, minute: i64) -> Option<i64> {
        let (year, month, dom) = civil_from_days(day);
        let dt = DateTime::new(
            i16::try_from(year).ok()?,
            i8::try_from(month).ok()?,
            i8::try_from(dom).ok()?,
            i8::try_from(minute / 60).ok()?,
            i8::try_from(minute % 60).ok()?,
            0,
            0,
        )
        .ok()?;
        self.zone
            .to_ambiguous_timestamp(dt)
            .compatible()
            .ok()
            .map(|at| at.as_millisecond())
    }

    fn within_until(&self, at: i64) -> bool {
        match self.until {
            None => true,
            Some(Until::Instant(limit)) => at <= limit,
            Some(Until::Local { day, second }) => self
                .local(at)
                .is_some_and(|(d, minute)| (d, minute * 60) <= (day, second)),
            Some(Until::Date(limit)) => self.local(at).is_some_and(|(d, _)| d <= limit),
        }
    }

    /// Whether the local day passes the BYDAY and BYMONTHDAY parts used as limits.
    fn day_passes(&self, day: i64) -> bool {
        if !self.by_day.is_empty() && !self.by_day.contains(&weekday(day)) {
            return false;
        }
        if !self.by_month_day.is_empty() {
            let (year, month, dom) = civil_from_days(day);
            let dim = days_in_month(year, month);
            return self
                .by_month_day
                .iter()
                .any(|wanted| *wanted == dom || *wanted + dim + 1 == dom);
        }
        true
    }

    /// For an hourly rule: the first instant of its series, the anchor's hour at the minute the
    /// rule names (the anchor's own minute when it names none), or the hour after when that is
    /// before the anchor.
    fn hourly_base(&self, anchor: i64) -> Option<i64> {
        let (day, minute) = self.local(anchor)?;
        let wanted = self.by_minute.unwrap_or(minute % 60);
        let base = self.instant(day, (minute / 60) * 60 + wanted)?;
        Some(if base < anchor { base + HOUR_MS } else { base })
    }

    /// The occurrences of period `k` of the rule, ascending.
    fn period(&self, k: i64, anchor: i64, d0: i64, minute0: i64) -> Option<Vec<i64>> {
        let tod =
            self.by_hour.unwrap_or(minute0 / 60) * 60 + self.by_minute.unwrap_or(minute0 % 60);
        let step = k.checked_mul(self.interval)?;
        let mut found = Vec::new();
        match self.freq {
            Freq::Hourly => {
                let at = self
                    .hourly_base(anchor)?
                    .checked_add(step.checked_mul(HOUR_MS)?)?;
                let (day, minute) = self.local(at)?;
                if self.day_passes(day) && self.by_hour.is_none_or(|hour| hour == minute / 60) {
                    found.push(at);
                }
            }
            Freq::Daily => {
                let day = d0.checked_add(step)?;
                if self.day_passes(day) {
                    found.extend(self.instant(day, tod));
                }
            }
            Freq::Weekly => {
                let week = d0 - weekday(d0) + step.checked_mul(7)?;
                let days: Vec<i64> = if self.by_day.is_empty() {
                    vec![weekday(d0)]
                } else {
                    self.by_day.clone()
                };
                for wd in days {
                    found.extend(self.instant(week + wd, tod));
                }
            }
            Freq::Monthly => {
                let index = month_index(d0).checked_add(step)?;
                let (year, month) = (index.div_euclid(12), index.rem_euclid(12) + 1);
                let dim = days_in_month(year, month);
                let mut doms: Vec<i64> = if !self.by_month_day.is_empty() {
                    self.by_month_day
                        .iter()
                        .map(|dom| if *dom > 0 { *dom } else { dim + 1 + *dom })
                        .collect()
                } else if !self.by_day.is_empty() {
                    (1..=dim).collect()
                } else {
                    vec![civil_from_days(d0).2]
                };
                doms.retain(|dom| (1..=dim).contains(dom));
                doms.retain(|dom| {
                    self.by_day.is_empty()
                        || self
                            .by_day
                            .contains(&weekday(days_from_civil(year, month, *dom)))
                });
                doms.sort_unstable();
                doms.dedup();
                for dom in doms {
                    found.extend(self.instant(days_from_civil(year, month, dom), tod));
                }
            }
        }
        found.sort_unstable();
        Some(found)
    }

    /// The first period that can hold an occurrence after `after`, never before the series
    /// starts: where the search begins when no COUNT makes it start at the first period.
    fn period_near(&self, anchor: i64, d0: i64, after: i64) -> i64 {
        let near = match self.freq {
            Freq::Hourly => self
                .hourly_base(anchor)
                .map(|base| (after - base).div_euclid(self.interval * HOUR_MS)),
            Freq::Daily => self
                .local(after)
                .map(|(day, _)| (day - d0).div_euclid(self.interval)),
            Freq::Weekly => self
                .local(after)
                .map(|(day, _)| (day - (d0 - weekday(d0))).div_euclid(7 * self.interval)),
            Freq::Monthly => self
                .local(after)
                .map(|(day, _)| (month_index(day) - month_index(d0)).div_euclid(self.interval)),
        };
        near.unwrap_or(0).saturating_sub(1).max(0)
    }

    /// The first occurrence strictly after `after`, for a series that starts at `anchor`, in
    /// milliseconds since the epoch; `None` when the rule has no occurrence left (its COUNT is
    /// spent or its UNTIL passed).
    pub fn next_after(&self, anchor: i64, after: i64) -> Option<i64> {
        let (d0, minute0) = self.local(anchor)?;
        let mut k = if self.count.is_some() {
            0
        } else {
            self.period_near(anchor, d0, after)
        };
        let mut seen = 0_u64;
        let mut empty = 0_u32;
        loop {
            let mut any = false;
            for at in self.period(k, anchor, d0, minute0)? {
                if at < anchor {
                    continue;
                }
                if !self.within_until(at) {
                    return None;
                }
                any = true;
                seen += 1;
                if self.count.is_some_and(|count| seen > count) {
                    return None;
                }
                if at > after {
                    return Some(at);
                }
            }
            if any {
                empty = 0;
            } else {
                empty += 1;
                if empty > MAX_EMPTY_PERIODS {
                    return None;
                }
            }
            k += 1;
        }
    }
}
