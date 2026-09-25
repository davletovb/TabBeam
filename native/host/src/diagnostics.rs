//! Structured lifecycle diagnostics (OBS-01).
//!
//! The host writes one JSON object per line to stderr. stdout carries only
//! Native Messaging frames, so diagnostics can never corrupt them. A record
//! names identifiers, timings, and outcomes, but never request content: no
//! prompt text, page context, unknown payload members, or raw frame bytes.
//! Identifiers taken from a request are truncated, and JSON escaping keeps each
//! record on one line whatever they contain.

use std::borrow::Cow;
use std::io::Write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::protocol::events::ErrorCode;

/// Longest identifier, in characters, copied from a request into a record.
pub const MAX_LOGGED_ID_CHARS: usize = 128;

/// Lifecycle events a record can describe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LifecycleEvent {
    #[serde(rename = "host.started")]
    HostStarted,
    #[serde(rename = "host.stopped")]
    HostStopped,
    /// A request failed validation and never reached a handler.
    #[serde(rename = "request.rejected")]
    RequestRejected,
    #[serde(rename = "request.completed")]
    RequestCompleted,
    #[serde(rename = "request.failed")]
    RequestFailed,
}

/// A normalized error (DOC-02): its code and reason, never its message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LoggedError {
    pub code: ErrorCode,
    pub reason: &'static str,
}

/// One diagnostics record. Unset fields are left out of the line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Record<'a> {
    pub event: LifecycleEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<LoggedError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_version: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected: Option<u64>,
}

impl Record<'_> {
    /// A record of `event` with every other field unset.
    pub const fn new(event: LifecycleEvent) -> Self {
        Self {
            event,
            request_id: None,
            method: None,
            provider_id: None,
            conversation_id: None,
            duration_ms: None,
            error: None,
            host_version: None,
            pid: None,
            reason: None,
            exit_code: None,
            requests: None,
            rejected: None,
        }
    }
}

#[derive(Serialize)]
struct Line<'a> {
    ts: &'a str,
    #[serde(flatten)]
    record: &'a Record<'a>,
}

/// Writes records to a sink, one JSON line each.
pub struct Diagnostics<W: Write> {
    sink: W,
    clock: fn() -> SystemTime,
}

impl<W: Write> Diagnostics<W> {
    pub fn new(sink: W) -> Self {
        Self {
            sink,
            clock: SystemTime::now,
        }
    }

    /// Uses `clock` for timestamps instead of the system clock.
    pub fn with_clock(sink: W, clock: fn() -> SystemTime) -> Self {
        Self { sink, clock }
    }

    /// Writes `record` as one line. A sink that fails, such as a closed stderr,
    /// is ignored: diagnostics never affect the Native Messaging session.
    pub fn record(&mut self, record: &Record<'_>) {
        let ts = format_timestamp((self.clock)());
        let Ok(mut line) = serde_json::to_vec(&Line { ts: &ts, record }) else {
            return;
        };
        line.push(b'\n');
        let _ = self.sink.write_all(&line);
        let _ = self.sink.flush();
    }

    pub fn into_inner(self) -> W {
        self.sink
    }
}

/// `text` cut to [`MAX_LOGGED_ID_CHARS`] characters, marked with `…` when cut.
pub fn loggable_id(text: Cow<'_, str>) -> Cow<'_, str> {
    match text.char_indices().nth(MAX_LOGGED_ID_CHARS) {
        None => text,
        Some((end, _)) => Cow::Owned(format!("{}…", &text[..end])),
    }
}

/// Whole milliseconds in `duration`, saturating.
pub fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// RFC 3339 UTC with milliseconds, such as `2026-09-25T01:23:45.678Z`. Times
/// before 1970 are written as the epoch.
fn format_timestamp(time: SystemTime) -> String {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = since_epoch.as_secs();
    let (year, month, day) = civil_from_days(seconds / 86_400);
    let second_of_day = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        second_of_day / 3_600,
        second_of_day % 3_600 / 60,
        second_of_day % 60,
        since_epoch.subsec_millis()
    )
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's
/// `civil_from_days`, restricted to non-negative day counts).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let days = days + 719_468;
    let era = days / 146_097;
    let day_of_era = days % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_clock() -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(1_758_763_425_678)
    }

    fn lines(diagnostics: Diagnostics<Vec<u8>>) -> Vec<serde_json::Value> {
        String::from_utf8(diagnostics.into_inner())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn timestamps_are_rfc3339_utc_with_milliseconds() {
        for (millis, expected) in [
            (0, "1970-01-01T00:00:00.000Z"),
            (951_782_400_000, "2000-02-29T00:00:00.000Z"),
            (951_868_799_999, "2000-02-29T23:59:59.999Z"),
            (1_758_763_425_678, "2025-09-25T01:23:45.678Z"),
            (4_107_542_400_000, "2100-03-01T00:00:00.000Z"),
            (253_402_300_799_000, "9999-12-31T23:59:59.000Z"),
        ] {
            assert_eq!(
                format_timestamp(UNIX_EPOCH + Duration::from_millis(millis)),
                expected
            );
        }
        let before_epoch = UNIX_EPOCH - Duration::from_secs(1);
        assert_eq!(format_timestamp(before_epoch), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn a_record_is_one_json_line_without_unset_fields() {
        let mut diagnostics = Diagnostics::with_clock(Vec::new(), fixed_clock);
        let mut record = Record::new(LifecycleEvent::RequestFailed);
        record.request_id = Some(Cow::Borrowed("req_1"));
        record.method = Some("conversation.send");
        record.duration_ms = Some(12);
        record.error = Some(LoggedError {
            code: ErrorCode::ProviderNotFound,
            reason: "PROVIDER_NOT_INSTALLED",
        });
        diagnostics.record(&record);

        let written = String::from_utf8(diagnostics.into_inner()).unwrap();
        assert_eq!(
            written,
            concat!(
                r#"{"ts":"2025-09-25T01:23:45.678Z","event":"request.failed","request_id":"req_1","#,
                r#""method":"conversation.send","duration_ms":12,"#,
                r#""error":{"code":"PROVIDER_NOT_FOUND","reason":"PROVIDER_NOT_INSTALLED"}}"#,
                "\n"
            )
        );
    }

    #[test]
    fn identifiers_cannot_break_a_line_or_grow_without_bound() {
        let mut diagnostics = Diagnostics::with_clock(Vec::new(), fixed_clock);
        let mut record = Record::new(LifecycleEvent::RequestCompleted);
        record.provider_id = Some(loggable_id(Cow::Borrowed(
            "fake\n{\"ts\":\"forged\",\"event\":\"host.stopped\"}",
        )));
        record.conversation_id = Some(loggable_id(Cow::Owned("é".repeat(500))));
        diagnostics.record(&record);

        let records = lines(diagnostics);
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0]["provider_id"],
            "fake\n{\"ts\":\"forged\",\"event\":\"host.stopped\"}"
        );
        let conversation_id = records[0]["conversation_id"].as_str().unwrap();
        assert_eq!(conversation_id.chars().count(), MAX_LOGGED_ID_CHARS + 1);
        assert!(conversation_id.ends_with('…'));
    }

    #[test]
    fn short_identifiers_are_kept_whole() {
        let exact = "a".repeat(MAX_LOGGED_ID_CHARS);
        assert_eq!(loggable_id(Cow::Borrowed(&exact)), exact.as_str());
        assert!(matches!(
            loggable_id(Cow::Borrowed("req")),
            Cow::Borrowed("req")
        ));
    }

    #[test]
    fn a_failing_sink_is_ignored() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("stderr is closed"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("stderr is closed"))
            }
        }

        let mut diagnostics = Diagnostics::new(Broken);
        diagnostics.record(&Record::new(LifecycleEvent::HostStarted));
    }
}
