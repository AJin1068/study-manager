//! Pure iCalendar (RFC 5545 subset) parser and generator for calendar plans. No I/O, no DB.
//!
//! Supported: VEVENT with DTSTART, DTEND/DURATION, SUMMARY, CATEGORIES, EXDATE, and
//! `RRULE:FREQ=WEEKLY[;BYDAY=..][;UNTIL=..]`. Floating local times and UTC (`Z`) single events are
//! accepted; every other event is skipped and reported by reason, never guessed at:
//! all-day events, `TZID` times (and `Z` times on a repeating event, whose weekdays would shift),
//! and any other recurrence feature.
//!
//! Hand-written instead of the `ical` crate: that crate only splits lines into raw properties (the
//! RRULE, time zone, and skip rules above would still be ours) and has no generator.

use std::fmt::Write as _;

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike};
use serde::Serialize;

use crate::error::{AppError, AppResult};

pub const MAX_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_EVENTS: usize = 5000;
const MAX_NAME_CHARS: usize = crate::plan::MAX_TEXT_CHARS;
const STAMP_FMT: &str = "%Y%m%dT%H%M%S";
const DAY_CODES: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

/// Why an event was not imported. One entry per skipped event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkipReason {
    AllDay,
    Timezone,
    UnsupportedRule,
    Invalid,
    /// Ended before today; decided by the importer.
    Past,
    /// Overlaps a plan that already exists or an earlier event of the file; decided by the importer.
    Overlap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IcsRepeat {
    /// Bit 0 = Monday .. bit 6 = Sunday, like `plans.repeat_weekdays`.
    pub weekdays: i64,
    pub until: Option<NaiveDate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IcsEvent {
    pub name: String,
    /// `CATEGORIES` (first value); the importer falls back to the name.
    pub subject: Option<String>,
    pub date: NaiveDate,
    pub time: NaiveTime,
    pub duration_min: Option<i64>,
    /// `X-SM-SETS`, written by our own export so a round trip keeps the set count exactly.
    pub sets: Option<i64>,
    pub repeat: Option<IcsRepeat>,
    pub exdates: Vec<NaiveDate>,
}

#[derive(Debug, Default)]
pub struct ParsedIcs {
    pub events: Vec<IcsEvent>,
    pub skipped: Vec<SkipReason>,
}

// ---- parsing ----

struct Prop {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Prop {
    fn param(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        match (
            line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')),
            lines.last_mut(),
        ) {
            (Some(rest), Some(prev)) => prev.push_str(rest),
            _ => lines.push(line.to_string()),
        }
    }
    lines
}

/// Splits at the first `sep` outside double quotes.
fn split_outside_quotes(text: &str, sep: char) -> Option<(&str, &str)> {
    let mut quoted = false;
    for (i, ch) in text.char_indices() {
        if ch == '"' {
            quoted = !quoted;
        } else if ch == sep && !quoted {
            return Some((&text[..i], &text[i + 1..]));
        }
    }
    None
}

fn parse_prop(line: &str) -> Option<Prop> {
    let (head, value) = split_outside_quotes(line, ':')?;
    let mut rest = head;
    let mut parts = Vec::new();
    while let Some((first, tail)) = split_outside_quotes(rest, ';') {
        parts.push(first);
        rest = tail;
    }
    parts.push(rest);
    let mut parts = parts.into_iter();
    let name = parts.next()?.trim().to_ascii_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| {
            (
                k.trim().to_ascii_uppercase(),
                v.trim().trim_matches('"').to_string(),
            )
        })
        .collect();
    Some(Prop {
        name,
        params,
        value: value.to_string(),
    })
}

fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push(' '),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn clean_label(raw: &str) -> Option<String> {
    let text = unescape(raw);
    let text = text.trim();
    (!text.is_empty()).then(|| text.chars().take(MAX_NAME_CHARS).collect())
}

enum Stamp {
    Floating(NaiveDateTime),
    Utc(NaiveDateTime),
    Date,
}

fn parse_stamp(prop: &Prop) -> Result<Stamp, SkipReason> {
    let value = prop.value.trim();
    if prop.param("VALUE") == Some("DATE") || value.len() == 8 {
        NaiveDate::parse_from_str(value, "%Y%m%d").map_err(|_| SkipReason::Invalid)?;
        return Ok(Stamp::Date);
    }
    if prop.param("TZID").is_some() {
        return Err(SkipReason::Timezone);
    }
    let (body, utc) = match value.strip_suffix('Z') {
        Some(body) => (body, true),
        None => (value, false),
    };
    if body.len() != 15 {
        return Err(SkipReason::Invalid);
    }
    let stamp = NaiveDateTime::parse_from_str(body, STAMP_FMT).map_err(|_| SkipReason::Invalid)?;
    Ok(if utc {
        Stamp::Utc(stamp)
    } else {
        Stamp::Floating(stamp)
    })
}

fn parse_duration_min(raw: &str) -> Option<i64> {
    let body = raw.trim().strip_prefix('P')?;
    let (mut total, mut num, mut in_time) = (0_i64, String::new(), false);
    for ch in body.chars() {
        match ch {
            'T' => in_time = true,
            '0'..='9' => num.push(ch),
            unit => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += match (unit, in_time) {
                    ('W', false) => n * 7 * 1440,
                    ('D', false) => n * 1440,
                    ('H', true) => n * 60,
                    ('M', true) => n,
                    ('S', true) => 0,
                    _ => return None,
                };
            }
        }
    }
    num.is_empty().then_some(total).filter(|m| *m > 0)
}

fn day_mask(code: &str) -> Option<i64> {
    DAY_CODES.iter().position(|c| *c == code).map(|i| 1 << i)
}

fn weekday_bit(date: NaiveDate) -> i64 {
    1 << date.weekday().num_days_from_monday()
}

/// `FREQ=WEEKLY` with plain weekday names; anything else is unsupported.
fn parse_rrule(
    raw: &str,
    start: NaiveDate,
    start_time: NaiveTime,
) -> Result<IcsRepeat, SkipReason> {
    let (mut weekly, mut weekdays, mut until) = (false, 0_i64, None);
    for part in raw.split(';').filter(|p| !p.trim().is_empty()) {
        let (key, value) = part.split_once('=').ok_or(SkipReason::UnsupportedRule)?;
        match key.trim().to_ascii_uppercase().as_str() {
            "FREQ" => weekly = value.trim().eq_ignore_ascii_case("WEEKLY"),
            "INTERVAL" if value.trim() == "1" => {}
            "WKST" => {}
            "BYDAY" => {
                for code in value.split(',') {
                    weekdays |= day_mask(&code.trim().to_ascii_uppercase())
                        .ok_or(SkipReason::UnsupportedRule)?;
                }
            }
            "UNTIL" => {
                let value = value.trim();
                let digits = value.get(..8).ok_or(SkipReason::Invalid)?;
                let mut date =
                    NaiveDate::parse_from_str(digits, "%Y%m%d").map_err(|_| SkipReason::Invalid)?;
                // A time-of-day bound earlier than the start time excludes that day's occurrence.
                if let Some(time) = value
                    .get(9..15)
                    .and_then(|t| NaiveTime::parse_from_str(t, "%H%M%S").ok())
                {
                    if time < start_time {
                        date -= Duration::days(1);
                    }
                }
                until = Some(date);
            }
            _ => return Err(SkipReason::UnsupportedRule),
        }
    }
    if !weekly {
        return Err(SkipReason::UnsupportedRule);
    }
    if weekdays == 0 {
        weekdays = weekday_bit(start);
    }
    if weekdays & weekday_bit(start) == 0 {
        return Err(SkipReason::UnsupportedRule);
    }
    if until.is_some_and(|u| u < start) {
        return Err(SkipReason::Invalid);
    }
    Ok(IcsRepeat { weekdays, until })
}

fn minute_floor(time: NaiveTime) -> NaiveTime {
    NaiveTime::from_hms_opt(time.hour(), time.minute(), 0).unwrap_or(time)
}

fn event_from_props<Tz: TimeZone>(props: &[Prop], tz: &Tz) -> Result<IcsEvent, SkipReason> {
    let find = |name: &str| props.iter().find(|p| p.name == name);
    if find("RECURRENCE-ID").is_some()
        || find("RDATE").is_some()
        || props.iter().filter(|p| p.name == "RRULE").count() > 1
    {
        return Err(SkipReason::UnsupportedRule);
    }
    if find("STATUS").is_some_and(|p| p.value.trim().eq_ignore_ascii_case("CANCELLED")) {
        return Err(SkipReason::Invalid);
    }
    let dtstart = find("DTSTART").ok_or(SkipReason::Invalid)?;
    let rrule = find("RRULE");
    let start = parse_stamp(dtstart)?;
    let (date, time) = match &start {
        Stamp::Date => return Err(SkipReason::AllDay),
        Stamp::Floating(naive) => (naive.date(), naive.time()),
        // A repeating UTC event keeps its UTC weekdays; converting only the start would misplace them.
        Stamp::Utc(_) if rrule.is_some() => return Err(SkipReason::Timezone),
        Stamp::Utc(naive) => {
            let local = chrono::Utc
                .from_utc_datetime(naive)
                .with_timezone(tz)
                .naive_local();
            (local.date(), local.time())
        }
    };
    let time = minute_floor(time);

    let duration_min = match (find("DTEND").and_then(|p| parse_stamp(p).ok()), &start) {
        (Some(Stamp::Floating(end)), Stamp::Floating(begin))
        | (Some(Stamp::Utc(end)), Stamp::Utc(begin)) => {
            Some((end - *begin).num_minutes()).filter(|m| *m > 0)
        }
        _ => find("DURATION").and_then(|p| parse_duration_min(&p.value)),
    };

    let name = clean_label(&find("SUMMARY").ok_or(SkipReason::Invalid)?.value)
        .ok_or(SkipReason::Invalid)?;
    let subject = find("CATEGORIES").and_then(|p| {
        split_unescaped_comma(&p.value)
            .into_iter()
            .find_map(|v| clean_label(&v))
    });
    let sets = find("X-SM-SETS")
        .and_then(|p| p.value.trim().parse::<i64>().ok())
        .filter(|n| (1..=24).contains(n));
    let repeat = rrule
        .map(|p| parse_rrule(&p.value, date, time))
        .transpose()?;

    let mut exdates = Vec::new();
    for prop in props.iter().filter(|p| p.name == "EXDATE") {
        for value in prop.value.split(',') {
            if let Some(day) = value
                .trim()
                .get(..8)
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y%m%d").ok())
            {
                exdates.push(day);
            }
        }
    }
    Ok(IcsEvent {
        name,
        subject,
        date,
        time,
        duration_min,
        sets,
        repeat,
        exdates,
    })
}

fn split_unescaped_comma(raw: &str) -> Vec<String> {
    let (mut parts, mut cur, mut escaped) = (Vec::new(), String::new(), false);
    for ch in raw.chars() {
        match (escaped, ch) {
            (false, '\\') => {
                escaped = true;
                cur.push(ch);
            }
            (false, ',') => parts.push(std::mem::take(&mut cur)),
            _ => {
                escaped = false;
                cur.push(ch);
            }
        }
    }
    parts.push(cur);
    parts
}

/// Parses an `.ics` text. Unsupported events are returned as skip reasons, not errors; only a
/// file that is not iCalendar at all (or is absurdly large) fails.
pub fn parse_ics<Tz: TimeZone>(text: &str, tz: &Tz) -> AppResult<ParsedIcs> {
    if text.len() > MAX_BYTES {
        return Err(AppError::Invalid("the calendar file is too large".into()));
    }
    let text = text.trim_start_matches('\u{feff}');
    let lines = unfold(text);
    if !lines
        .iter()
        .any(|l| l.trim().eq_ignore_ascii_case("BEGIN:VCALENDAR"))
    {
        return Err(AppError::Invalid("not an iCalendar (.ics) file".into()));
    }
    let mut parsed = ParsedIcs::default();
    let mut current: Option<Vec<Prop>> = None;
    let mut nested = 0_usize;
    let mut seen = 0_usize;
    for line in &lines {
        let upper = line.trim().to_ascii_uppercase();
        if upper == "BEGIN:VEVENT" {
            current = Some(Vec::new());
            nested = 0;
        } else if upper == "END:VEVENT" {
            if let Some(props) = current.take() {
                seen += 1;
                if seen > MAX_EVENTS {
                    return Err(AppError::Invalid(format!(
                        "the calendar has more than {MAX_EVENTS} events"
                    )));
                }
                match event_from_props(&props, tz) {
                    Ok(event) => parsed.events.push(event),
                    Err(reason) => parsed.skipped.push(reason),
                }
            }
        } else if current.is_some() {
            if upper.starts_with("BEGIN:") {
                nested += 1;
            } else if upper.starts_with("END:") {
                nested = nested.saturating_sub(1);
            } else if nested == 0 {
                if let (Some(props), Some(prop)) = (current.as_mut(), parse_prop(line)) {
                    props.push(prop);
                }
            }
        }
    }
    Ok(parsed)
}

// ---- generation ----

/// One event to write. Times are floating local times, the same convention plans use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportEvent {
    pub uid: String,
    pub name: String,
    pub subject: String,
    pub date: NaiveDate,
    pub time: NaiveTime,
    pub duration_min: i64,
    pub sets: i64,
    pub repeat: Option<IcsRepeat>,
    pub exdates: Vec<NaiveDate>,
}

fn escape_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\\' | ';' | ',' => {
                out.push('\\');
                out.push(ch);
            }
            '\n' => out.push_str("\\n"),
            '\r' => {}
            other => out.push(other),
        }
    }
    out
}

/// Content lines are folded at 75 octets, never inside a UTF-8 sequence.
fn push_line(out: &mut String, line: &str) {
    let mut octets = 0;
    for ch in line.chars() {
        let len = ch.len_utf8();
        if octets + len > 75 {
            out.push_str("\r\n ");
            octets = 1;
        }
        out.push(ch);
        octets += len;
    }
    out.push_str("\r\n");
}

fn stamp(date: NaiveDate, time: NaiveTime) -> String {
    date.and_time(time).format(STAMP_FMT).to_string()
}

fn byday(weekdays: i64) -> String {
    DAY_CODES
        .iter()
        .enumerate()
        .filter(|(i, _)| weekdays & (1 << i) != 0)
        .map(|(_, c)| *c)
        .collect::<Vec<_>>()
        .join(",")
}

/// Writes a VCALENDAR. `now` is the DTSTAMP (UTC).
pub fn generate_ics(events: &[ExportEvent], now: NaiveDateTime) -> String {
    let mut out = String::new();
    push_line(&mut out, "BEGIN:VCALENDAR");
    push_line(&mut out, "VERSION:2.0");
    push_line(&mut out, "PRODID:-//Study Manager//Plans//KO");
    push_line(&mut out, "CALSCALE:GREGORIAN");
    let dtstamp = format!("{}Z", now.format(STAMP_FMT));
    for event in events {
        push_line(&mut out, "BEGIN:VEVENT");
        push_line(&mut out, &format!("UID:{}", event.uid));
        push_line(&mut out, &format!("DTSTAMP:{dtstamp}"));
        push_line(
            &mut out,
            &format!("DTSTART:{}", stamp(event.date, event.time)),
        );
        let end = event.date.and_time(event.time) + Duration::minutes(event.duration_min);
        push_line(&mut out, &format!("DTEND:{}", end.format(STAMP_FMT)));
        push_line(&mut out, &format!("SUMMARY:{}", escape_text(&event.name)));
        push_line(
            &mut out,
            &format!("CATEGORIES:{}", escape_text(&event.subject)),
        );
        push_line(&mut out, &format!("X-SM-SETS:{}", event.sets));
        if let Some(repeat) = &event.repeat {
            let mut rule = format!("RRULE:FREQ=WEEKLY;BYDAY={}", byday(repeat.weekdays));
            if let Some(until) = repeat.until {
                // Floating, like DTSTART; the last day is included.
                let _ = write!(
                    rule,
                    ";UNTIL={}",
                    stamp(
                        until,
                        NaiveTime::from_hms_opt(23, 59, 59).unwrap_or(NaiveTime::MIN)
                    )
                );
            }
            push_line(&mut out, &rule);
        }
        if !event.exdates.is_empty() {
            let days: Vec<String> = event
                .exdates
                .iter()
                .map(|d| stamp(*d, event.time))
                .collect();
            push_line(&mut out, &format!("EXDATE:{}", days.join(",")));
        }
        push_line(&mut out, "END:VEVENT");
    }
    push_line(&mut out, "END:VCALENDAR");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use chrono_tz::America::New_York;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).expect("date")
    }

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).expect("time")
    }

    fn wrap(events: &str) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{events}END:VCALENDAR\r\n")
    }

    fn vevent(body: &str) -> String {
        format!("BEGIN:VEVENT\r\nUID:x\r\n{body}END:VEVENT\r\n")
    }

    fn parse_one(body: &str) -> ParsedIcs {
        parse_ics(&wrap(&vevent(body)), &Utc).expect("parse")
    }

    #[test]
    fn parses_floating_single_event_with_duration_from_dtend() {
        let parsed =
            parse_one("DTSTART:20261105T200000\r\nDTEND:20261105T230000\r\nSUMMARY:수학\r\n");
        assert!(parsed.skipped.is_empty());
        let e = &parsed.events[0];
        assert_eq!(
            (e.name.as_str(), e.date, e.time, e.duration_min),
            ("수학", d(2026, 11, 5), t(20, 0), Some(180))
        );
        assert_eq!(e.repeat, None);
    }

    #[test]
    fn weekly_rule_exdate_and_categories_are_read() {
        let parsed = parse_one(
            "DTSTART:20261102T200000\r\nDURATION:PT1H30M\r\nSUMMARY:영어\\, 문법\r\nCATEGORIES:영어\r\n\
             RRULE:FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20261231T235959\r\nEXDATE:20261104T200000,20261109T200000\r\nX-SM-SETS:2\r\n",
        );
        let e = &parsed.events[0];
        assert_eq!(e.name, "영어, 문법");
        assert_eq!(e.subject.as_deref(), Some("영어"));
        assert_eq!(e.duration_min, Some(90));
        assert_eq!(e.sets, Some(2));
        assert_eq!(
            e.repeat,
            Some(IcsRepeat {
                weekdays: 0b101,
                until: Some(d(2026, 12, 31))
            })
        );
        assert_eq!(e.exdates, [d(2026, 11, 4), d(2026, 11, 9)]);
    }

    #[test]
    fn byday_defaults_to_the_start_weekday() {
        let parsed = parse_one("DTSTART:20261105T200000\r\nSUMMARY:a\r\nRRULE:FREQ=WEEKLY\r\n");
        assert_eq!(
            parsed.events[0].repeat,
            Some(IcsRepeat {
                weekdays: 1 << 3,
                until: None
            })
        );
    }

    #[test]
    fn unsupported_inputs_are_reported_by_reason() {
        let cases: Vec<(&str, SkipReason)> = vec![
            (
                "DTSTART;VALUE=DATE:20261105\r\nSUMMARY:a\r\n",
                SkipReason::AllDay,
            ),
            (
                "DTSTART;TZID=Asia/Seoul:20261105T200000\r\nSUMMARY:a\r\n",
                SkipReason::Timezone,
            ),
            (
                "DTSTART:20261105T200000Z\r\nSUMMARY:a\r\nRRULE:FREQ=WEEKLY\r\n",
                SkipReason::Timezone,
            ),
            (
                "DTSTART:20261105T200000\r\nSUMMARY:a\r\nRRULE:FREQ=DAILY\r\n",
                SkipReason::UnsupportedRule,
            ),
            (
                "DTSTART:20261105T200000\r\nSUMMARY:a\r\nRRULE:FREQ=WEEKLY;INTERVAL=2\r\n",
                SkipReason::UnsupportedRule,
            ),
            (
                "DTSTART:20261105T200000\r\nSUMMARY:a\r\nRRULE:FREQ=WEEKLY;COUNT=4\r\n",
                SkipReason::UnsupportedRule,
            ),
            (
                "DTSTART:20261105T200000\r\nSUMMARY:a\r\nRRULE:FREQ=WEEKLY;BYDAY=1MO\r\n",
                SkipReason::UnsupportedRule,
            ),
            (
                "DTSTART:20261105T200000\r\nSUMMARY:a\r\nRRULE:FREQ=WEEKLY;BYDAY=MO\r\n",
                SkipReason::UnsupportedRule,
            ),
            (
                "DTSTART:20261105T200000\r\nSUMMARY:a\r\nRECURRENCE-ID:20261105T200000\r\n",
                SkipReason::UnsupportedRule,
            ),
            (
                "DTSTART:20261105T200000\r\nSUMMARY:a\r\nRDATE:20261106T200000\r\n",
                SkipReason::UnsupportedRule,
            ),
            (
                "DTSTART:20261105T200000\r\nSUMMARY:a\r\nSTATUS:CANCELLED\r\n",
                SkipReason::Invalid,
            ),
            ("SUMMARY:a\r\n", SkipReason::Invalid),
            ("DTSTART:20261105T200000\r\n", SkipReason::Invalid),
            (
                "DTSTART:20261105T200000\r\nSUMMARY:  \r\n",
                SkipReason::Invalid,
            ),
            (
                "DTSTART:20261305T200000\r\nSUMMARY:a\r\n",
                SkipReason::Invalid,
            ),
        ];
        for (body, reason) in cases {
            let parsed = parse_one(body);
            assert!(parsed.events.is_empty(), "{body:?}");
            assert_eq!(parsed.skipped, [reason], "{body:?}");
        }
    }

    #[test]
    fn utc_single_event_converts_to_the_local_zone() {
        let text = wrap(&vevent(
            "DTSTART:20261105T013000Z\r\nDTEND:20261105T033000Z\r\nSUMMARY:a\r\n",
        ));
        let e = &parse_ics(&text, &New_York).expect("parse").events[0];
        assert_eq!(
            (e.date, e.time, e.duration_min),
            (d(2026, 11, 4), t(20, 30), Some(120))
        );
    }

    #[test]
    fn folded_lines_alarms_and_escapes_are_handled() {
        let body = "DTSTART:20261105T200000\r\nSUMMARY:긴 제목을 접은 줄\r\n  로 이어짐\r\nBEGIN:VALARM\r\nSUMMARY:alarm\r\nTRIGGER:-PT5M\r\nEND:VALARM\r\n";
        let parsed = parse_one(body);
        assert_eq!(parsed.events[0].name, "긴 제목을 접은 줄 로 이어짐");
    }

    #[test]
    fn not_an_ics_file_and_oversize_fail() {
        assert!(parse_ics("hello", &Utc).is_err());
        assert!(parse_ics(&"x".repeat(MAX_BYTES + 1), &Utc).is_err());
        let many = wrap(&vevent("DTSTART:20261105T200000\r\nSUMMARY:a\r\n").repeat(MAX_EVENTS + 1));
        assert!(parse_ics(&many, &Utc).is_err());
    }

    fn sample_events() -> Vec<ExportEvent> {
        vec![
            ExportEvent {
                uid: "sm-plan-1@study-manager.local".into(),
                name: "수학, 심화; 반".into(),
                subject: "수학".into(),
                date: d(2026, 11, 2),
                time: t(20, 0),
                duration_min: 180,
                sets: 3,
                repeat: Some(IcsRepeat {
                    weekdays: 0b101,
                    until: Some(d(2026, 12, 31)),
                }),
                exdates: vec![d(2026, 11, 4)],
            },
            ExportEvent {
                uid: "sm-plan-2@study-manager.local".into(),
                name: "가".repeat(100),
                subject: "국어".into(),
                date: d(2026, 11, 6),
                time: t(9, 5),
                duration_min: 25,
                sets: 1,
                repeat: None,
                exdates: vec![],
            },
        ]
    }

    #[test]
    fn generated_text_round_trips_and_is_folded() {
        let now = d(2026, 11, 6).and_hms_opt(1, 2, 3).expect("now");
        let text = generate_ics(&sample_events(), now);
        assert!(text.starts_with("BEGIN:VCALENDAR\r\n") && text.ends_with("END:VCALENDAR\r\n"));
        assert!(text.contains("DTSTAMP:20261106T010203Z"));
        for line in text.split("\r\n") {
            assert!(line.len() <= 75, "unfolded line: {line}");
        }
        let parsed = parse_ics(&text, &Utc).expect("parse");
        assert!(parsed.skipped.is_empty());
        assert_eq!(parsed.events.len(), 2);
        let (a, b) = (&parsed.events[0], &parsed.events[1]);
        assert_eq!(
            (
                a.name.as_str(),
                a.subject.as_deref(),
                a.sets,
                a.duration_min
            ),
            ("수학, 심화; 반", Some("수학"), Some(3), Some(180))
        );
        assert_eq!(
            a.repeat,
            Some(IcsRepeat {
                weekdays: 0b101,
                until: Some(d(2026, 12, 31))
            })
        );
        assert_eq!(a.exdates, [d(2026, 11, 4)]);
        assert_eq!(b.name.chars().count(), 100);
        assert_eq!(
            (b.date, b.time, b.repeat.clone()),
            (d(2026, 11, 6), t(9, 5), None)
        );
    }

    #[test]
    fn thousand_events_parse_quickly() {
        let events: Vec<ExportEvent> = (0..1000)
            .map(|i| ExportEvent {
                uid: format!("u{i}"),
                name: format!("일정 {i}"),
                subject: "과목".into(),
                date: d(2026, 1, 1) + Duration::days(i % 300),
                time: t(8 + (i % 12) as u32, 0),
                duration_min: 50,
                sets: 1,
                repeat: (i % 2 == 0).then(|| IcsRepeat {
                    weekdays: weekday_bit(d(2026, 1, 1) + Duration::days(i % 300)),
                    until: None,
                }),
                exdates: vec![],
            })
            .collect();
        let text = generate_ics(&events, d(2026, 1, 1).and_hms_opt(0, 0, 0).expect("now"));
        let started = std::time::Instant::now();
        let parsed = parse_ics(&text, &Utc).expect("parse");
        assert_eq!(parsed.events.len(), 1000);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
    }
}
