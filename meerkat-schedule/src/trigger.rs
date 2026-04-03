use crate::error::ScheduleDomainError;
use crate::types::{CalendarFieldSpec, CalendarTriggerSpec, IntervalTriggerSpec, TriggerSpec};
use chrono::{DateTime, Datelike, Duration, LocalResult, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronAuthoringSpec {
    pub expression: String,
    pub timezone: String,
}

impl CronAuthoringSpec {
    pub fn into_calendar(self) -> Result<CalendarTriggerSpec, ScheduleDomainError> {
        let fields: Vec<&str> = self.expression.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(ScheduleDomainError::InvalidCron(
                "cron expressions must have 5 fields: minute hour day_of_month month day_of_week"
                    .to_string(),
            ));
        }

        Ok(CalendarTriggerSpec {
            timezone: normalize_timezone(&self.timezone)?.name().to_string(),
            minute: parse_field(fields[0], 0, 59, ValueDomain::Minute)?,
            hour: parse_field(fields[1], 0, 23, ValueDomain::Hour)?,
            day_of_month: parse_field(fields[2], 1, 31, ValueDomain::DayOfMonth)?,
            month: parse_field(fields[3], 1, 12, ValueDomain::Month)?,
            day_of_week: parse_field(fields[4], 0, 6, ValueDomain::DayOfWeek)?,
            year: None,
        })
    }

    pub fn into_trigger(self) -> Result<TriggerSpec, ScheduleDomainError> {
        Ok(TriggerSpec::Calendar(self.into_calendar()?))
    }
}

pub fn next_due_after(
    trigger: &TriggerSpec,
    after_utc: Option<DateTime<Utc>>,
) -> Result<Option<DateTime<Utc>>, ScheduleDomainError> {
    match trigger {
        TriggerSpec::Once { due_at_utc } => {
            if after_utc.is_none_or(|after| *due_at_utc > after) {
                Ok(Some(*due_at_utc))
            } else {
                Ok(None)
            }
        }
        TriggerSpec::Interval(spec) => next_interval_due_after(spec, after_utc),
        TriggerSpec::Calendar(spec) => next_calendar_due_after(spec, after_utc),
    }
}

pub fn occurrences_for_horizon(
    trigger: &TriggerSpec,
    cursor_utc: Option<DateTime<Utc>>,
    horizon_end_utc: DateTime<Utc>,
    max_occurrences: usize,
) -> Result<Vec<DateTime<Utc>>, ScheduleDomainError> {
    if max_occurrences == 0 {
        return Ok(Vec::new());
    }

    if let TriggerSpec::Calendar(spec) = trigger {
        let tz = normalize_timezone(&spec.timezone)?;
        let search_start =
            next_minute_boundary(cursor_utc.unwrap_or_else(|| Utc::now() - Duration::minutes(1)));
        return scan_calendar(spec, tz, search_start, horizon_end_utc, max_occurrences);
    }

    let mut occurrences = Vec::new();
    let mut cursor = cursor_utc;

    while occurrences.len() < max_occurrences {
        let Some(next_due) = next_due_after(trigger, cursor)? else {
            break;
        };
        if next_due > horizon_end_utc {
            break;
        }
        occurrences.push(next_due);
        cursor = Some(next_due);
    }

    Ok(occurrences)
}

fn next_interval_due_after(
    spec: &IntervalTriggerSpec,
    after_utc: Option<DateTime<Utc>>,
) -> Result<Option<DateTime<Utc>>, ScheduleDomainError> {
    if spec.every_seconds == 0 {
        return Err(ScheduleDomainError::InvalidTrigger(
            "interval trigger requires every_seconds >= 1".to_string(),
        ));
    }

    let step_seconds = i64::try_from(spec.every_seconds).map_err(|_| {
        ScheduleDomainError::InvalidTrigger("interval every_seconds exceeds i64 range".to_string())
    })?;

    let candidate = match after_utc {
        None => spec.start_at_utc,
        Some(after) if after < spec.start_at_utc => spec.start_at_utc,
        Some(after) => {
            let elapsed = after.signed_duration_since(spec.start_at_utc);
            let mut intervals = elapsed.num_seconds().div_euclid(step_seconds) + 1;
            let mut candidate = spec.start_at_utc + Duration::seconds(intervals * step_seconds);
            while candidate <= after {
                intervals += 1;
                candidate = spec.start_at_utc + Duration::seconds(intervals * step_seconds);
            }
            candidate
        }
    };

    if spec.end_at_utc.is_some_and(|end_at| candidate > end_at) {
        Ok(None)
    } else {
        Ok(Some(candidate))
    }
}

fn next_calendar_due_after(
    spec: &CalendarTriggerSpec,
    after_utc: Option<DateTime<Utc>>,
) -> Result<Option<DateTime<Utc>>, ScheduleDomainError> {
    let tz = normalize_timezone(&spec.timezone)?;
    let search_start = next_minute_boundary(after_utc.unwrap_or_else(Utc::now));
    let search_end = search_start + Duration::days(366 * 5);

    scan_calendar(spec, tz, search_start, search_end, 1).map(|mut matches| matches.pop())
}

fn scan_calendar(
    spec: &CalendarTriggerSpec,
    tz: Tz,
    start_utc: DateTime<Utc>,
    end_utc: DateTime<Utc>,
    max_matches: usize,
) -> Result<Vec<DateTime<Utc>>, ScheduleDomainError> {
    if end_utc < start_utc {
        return Ok(Vec::new());
    }

    let mut matches = Vec::new();
    let mut cursor = truncate_to_minute(start_utc);

    while cursor <= end_utc && matches.len() < max_matches {
        let local = cursor.with_timezone(&tz);
        if matches_calendar(spec, &local) {
            matches.push(cursor);
        }
        cursor += Duration::minutes(1);
    }

    Ok(matches)
}

fn matches_calendar(spec: &CalendarTriggerSpec, local: &chrono::DateTime<Tz>) -> bool {
    spec.minute.contains(local.minute())
        && spec.hour.contains(local.hour())
        && spec.day_of_month.contains(local.day())
        && spec.month.contains(local.month())
        && spec
            .day_of_week
            .contains(local.weekday().num_days_from_sunday())
        && spec
            .year
            .as_ref()
            .is_none_or(|years| years.contains(local.year_ce().1))
}

fn truncate_to_minute(input: DateTime<Utc>) -> DateTime<Utc> {
    match Utc.with_ymd_and_hms(
        input.year(),
        input.month(),
        input.day(),
        input.hour(),
        input.minute(),
        0,
    ) {
        LocalResult::Single(value) => value,
        _ => input,
    }
}

fn next_minute_boundary(after: DateTime<Utc>) -> DateTime<Utc> {
    let truncated = truncate_to_minute(after);
    if truncated <= after {
        truncated + Duration::minutes(1)
    } else {
        truncated
    }
}

fn normalize_timezone(value: &str) -> Result<Tz, ScheduleDomainError> {
    value
        .parse::<Tz>()
        .map_err(|_| ScheduleDomainError::InvalidTrigger(format!("invalid IANA timezone: {value}")))
}

#[derive(Clone, Copy)]
enum ValueDomain {
    Minute,
    Hour,
    DayOfMonth,
    Month,
    DayOfWeek,
}

fn parse_field(
    raw: &str,
    min: u32,
    max: u32,
    domain: ValueDomain,
) -> Result<CalendarFieldSpec, ScheduleDomainError> {
    if raw == "*" {
        return Ok(CalendarFieldSpec::Any);
    }

    let mut values = BTreeSet::new();
    for part in raw.split(',') {
        parse_part(part.trim(), min, max, domain, &mut values)?;
    }

    if values.is_empty() {
        return Err(ScheduleDomainError::InvalidCron(format!(
            "cron field `{raw}` expands to no values"
        )));
    }

    Ok(CalendarFieldSpec::Values(values.into_iter().collect()))
}

fn parse_part(
    raw: &str,
    min: u32,
    max: u32,
    domain: ValueDomain,
    out: &mut BTreeSet<u32>,
) -> Result<(), ScheduleDomainError> {
    let (base, step) = match raw.split_once('/') {
        Some((base, step)) => (base, Some(parse_u32(step, domain)?)),
        None => (raw, None),
    };

    if step == Some(0) {
        return Err(ScheduleDomainError::InvalidCron(format!(
            "cron step must be >= 1 in `{raw}`"
        )));
    }

    let (range_start, range_end) = if base == "*" {
        (min, max)
    } else if let Some((start, end)) = base.split_once('-') {
        (
            normalize_field_value(start, domain)?,
            normalize_field_value(end, domain)?,
        )
    } else {
        let value = normalize_field_value(base, domain)?;
        (value, value)
    };

    if range_start < min || range_end > max || range_start > range_end {
        return Err(ScheduleDomainError::InvalidCron(format!(
            "cron field `{raw}` is outside the allowed range {min}..={max}"
        )));
    }

    let step = step.unwrap_or(1);
    let mut value = range_start;
    while value <= range_end {
        out.insert(value);
        match value.checked_add(step) {
            Some(next) => value = next,
            None => break,
        }
    }
    Ok(())
}

fn normalize_field_value(raw: &str, domain: ValueDomain) -> Result<u32, ScheduleDomainError> {
    let upper = raw.trim().to_ascii_uppercase();
    let parsed = match domain {
        ValueDomain::Month => month_name(&upper).or_else(|| upper.parse::<u32>().ok()),
        ValueDomain::DayOfWeek => day_of_week_name(&upper).or_else(|| upper.parse::<u32>().ok()),
        _ => upper.parse::<u32>().ok(),
    }
    .ok_or_else(|| ScheduleDomainError::InvalidCron(format!("invalid cron token `{raw}`")))?;

    if matches!(domain, ValueDomain::DayOfWeek) && parsed == 7 {
        Ok(0)
    } else {
        Ok(parsed)
    }
}

fn parse_u32(raw: &str, domain: ValueDomain) -> Result<u32, ScheduleDomainError> {
    normalize_field_value(raw, domain)
}

fn month_name(value: &str) -> Option<u32> {
    match value {
        "JAN" => Some(1),
        "FEB" => Some(2),
        "MAR" => Some(3),
        "APR" => Some(4),
        "MAY" => Some(5),
        "JUN" => Some(6),
        "JUL" => Some(7),
        "AUG" => Some(8),
        "SEP" => Some(9),
        "OCT" => Some(10),
        "NOV" => Some(11),
        "DEC" => Some(12),
        _ => None,
    }
}

fn day_of_week_name(value: &str) -> Option<u32> {
    match value {
        "SUN" => Some(0),
        "MON" => Some(1),
        "TUE" | "TUES" => Some(2),
        "WED" => Some(3),
        "THU" | "THUR" | "THURS" => Some(4),
        "FRI" => Some(5),
        "SAT" => Some(6),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn cron_codec_normalizes_calendar_trigger() {
        let spec = CronAuthoringSpec {
            expression: "*/15 9-17 * * MON-FRI".to_string(),
            timezone: "Europe/Stockholm".to_string(),
        };

        let calendar = spec.into_calendar().expect("cron should parse");
        assert!(matches!(
            calendar.minute,
            CalendarFieldSpec::Values(ref values)
                if values == &[0, 15, 30, 45]
        ));
        assert!(matches!(
            calendar.hour,
            CalendarFieldSpec::Values(ref values)
                if values == &[9, 10, 11, 12, 13, 14, 15, 16, 17]
        ));
    }

    #[test]
    fn interval_trigger_uses_strictly_future_occurrences() {
        let spec = IntervalTriggerSpec {
            start_at_utc: Utc.with_ymd_and_hms(2026, 4, 2, 9, 0, 0).unwrap(),
            every_seconds: 300,
            end_at_utc: None,
        };

        let due = next_interval_due_after(
            &spec,
            Some(Utc.with_ymd_and_hms(2026, 4, 2, 9, 10, 0).unwrap()),
        )
        .expect("interval should evaluate");

        assert_eq!(
            due,
            Some(Utc.with_ymd_and_hms(2026, 4, 2, 9, 15, 0).unwrap())
        );
    }

    #[test]
    fn calendar_trigger_handles_dst_by_scanning_utc_minutes() {
        let trigger = TriggerSpec::Calendar(CalendarTriggerSpec {
            timezone: "Europe/Stockholm".to_string(),
            minute: CalendarFieldSpec::Values(vec![30]),
            hour: CalendarFieldSpec::Values(vec![2]),
            day_of_month: CalendarFieldSpec::Any,
            month: CalendarFieldSpec::Any,
            day_of_week: CalendarFieldSpec::Any,
            year: None,
        });

        let after = Utc.with_ymd_and_hms(2026, 3, 28, 23, 0, 0).unwrap();
        let due = next_due_after(&trigger, Some(after)).expect("calendar scan should succeed");

        assert!(due.is_some());
    }
}
