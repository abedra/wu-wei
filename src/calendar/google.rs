use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone, Utc};
use serde::Deserialize;

use super::CalendarEvent;

const EVENTS_URL: &str = "https://www.googleapis.com/calendar/v3/calendars/primary/events";

#[derive(Deserialize)]
struct EventsResponse {
    #[serde(default)]
    items: Vec<RawEvent>,
}

#[derive(Deserialize)]
struct RawEvent {
    id: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    location: Option<String>,
    start: RawEventTime,
    end: RawEventTime,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Deserialize)]
struct RawEventTime {
    date: Option<String>,
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
}

impl RawEventTime {
    /// A timed event gives `dateTime`; an all-day event gives a bare `date`
    /// instead, which is anchored to local midnight here so it still sorts
    /// sensibly alongside timed events on the same day — `all_day` is what
    /// tells the UI to show "All day" rather than that synthetic time.
    fn to_datetime(&self) -> Option<(DateTime<Utc>, bool)> {
        if let Some(date_time) = &self.date_time {
            let parsed = DateTime::parse_from_rfc3339(date_time).ok()?;
            return Some((parsed.with_timezone(&Utc), false));
        }
        let date = self.date.as_ref()?;
        let naive = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
        let midnight = naive.and_hms_opt(0, 0, 0)?;
        let local = Local.from_local_datetime(&midnight).single()?;
        Some((local.with_timezone(&Utc), true))
    }
}

/// Fetches `today`'s events (local midnight to the next local midnight) from
/// the connected Google account's primary calendar. Blocking by design —
/// always called from `calendar::run_async`'s background thread, mirroring
/// how `llm::Provider` implementations are blocking (see `src/llm/mod.rs`).
pub fn fetch_today_events(
    access_token: &str,
    today: NaiveDate,
) -> Result<Vec<CalendarEvent>, String> {
    let start_of_day = today
        .and_hms_opt(0, 0, 0)
        .and_then(|dt| Local.from_local_datetime(&dt).single())
        .ok_or_else(|| "couldn't resolve today's local time window".to_string())?;
    let end_of_day = start_of_day + Duration::days(1);

    let mut response = ureq::get(EVENTS_URL)
        .query("timeMin", start_of_day.to_rfc3339())
        .query("timeMax", end_of_day.to_rfc3339())
        .query("singleEvents", "true")
        .query("orderBy", "startTime")
        .header("Authorization", format!("Bearer {access_token}"))
        .call()
        .map_err(|e| format!("Google Calendar request failed: {e}"))?;

    let parsed: EventsResponse = response
        .body_mut()
        .read_json()
        .map_err(|e| format!("failed to read Google Calendar response: {e}"))?;

    Ok(parsed
        .items
        .into_iter()
        .filter_map(raw_event_to_calendar_event)
        .collect())
}

fn raw_event_to_calendar_event(item: RawEvent) -> Option<CalendarEvent> {
    if item.status.as_deref() == Some("cancelled") {
        return None;
    }
    let (start, all_day) = item.start.to_datetime()?;
    let (end, _) = item.end.to_datetime()?;
    Some(CalendarEvent {
        id: item.id,
        title: item
            .summary
            .unwrap_or_else(|| "(untitled event)".to_string()),
        start,
        end,
        all_day,
        location: item.location,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timed(date_time: &str) -> RawEventTime {
        RawEventTime {
            date: None,
            date_time: Some(date_time.to_string()),
        }
    }

    fn all_day(date: &str) -> RawEventTime {
        RawEventTime {
            date: Some(date.to_string()),
            date_time: None,
        }
    }

    #[test]
    fn maps_a_timed_event() {
        let raw = RawEvent {
            id: "abc".to_string(),
            summary: Some("Standup".to_string()),
            location: Some("Zoom".to_string()),
            start: timed("2026-08-22T09:00:00-05:00"),
            end: timed("2026-08-22T09:30:00-05:00"),
            status: Some("confirmed".to_string()),
        };
        let event = raw_event_to_calendar_event(raw).unwrap();
        assert_eq!(event.id, "abc");
        assert_eq!(event.title, "Standup");
        assert!(!event.all_day);
        assert_eq!(event.location.as_deref(), Some("Zoom"));
    }

    #[test]
    fn maps_an_all_day_event() {
        let raw = RawEvent {
            id: "xyz".to_string(),
            summary: Some("Company Holiday".to_string()),
            location: None,
            start: all_day("2026-08-22"),
            end: all_day("2026-08-23"),
            status: None,
        };
        let event = raw_event_to_calendar_event(raw).unwrap();
        assert!(event.all_day);
    }

    #[test]
    fn drops_cancelled_events() {
        let raw = RawEvent {
            id: "cancelled".to_string(),
            summary: Some("Old meeting".to_string()),
            location: None,
            start: timed("2026-08-22T09:00:00Z"),
            end: timed("2026-08-22T09:30:00Z"),
            status: Some("cancelled".to_string()),
        };
        assert!(raw_event_to_calendar_event(raw).is_none());
    }

    #[test]
    fn defaults_a_missing_title() {
        let raw = RawEvent {
            id: "no-title".to_string(),
            summary: None,
            location: None,
            start: timed("2026-08-22T09:00:00Z"),
            end: timed("2026-08-22T09:30:00Z"),
            status: None,
        };
        let event = raw_event_to_calendar_event(raw).unwrap();
        assert_eq!(event.title, "(untitled event)");
    }
}
