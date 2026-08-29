mod google;
mod oauth;

pub use oauth::{GoogleTokenSet, connect_async};

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use chrono::{DateTime, Duration, NaiveDate, Utc};

use crate::db::settings_repo;

/// One event on the connected Google Calendar for a single day's fetch (see
/// `run_async`) — never persisted locally, re-fetched on every sync.
#[derive(Debug, Clone, PartialEq)]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// Google gave a `date` (a whole day), not a `dateTime` — `start`/`end`
    /// are still populated (anchored to local midnight) so an all-day event
    /// sorts sensibly alongside timed ones, but the UI shows "All day"
    /// instead of a time for it.
    pub all_day: bool,
    pub location: Option<String>,
}

/// OAuth + Calendar API credentials, resolved from the `settings` table the
/// same way `LlmConfig::resolve` resolves LLM credentials (see
/// `src/llm/mod.rs`) — `None` means the calendar integration simply hasn't
/// been connected yet (see `AppState::start_google_oauth`), not an error.
#[derive(Debug, Clone)]
pub struct GoogleCalendarConfig {
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
    pub access_token: String,
    pub expiry: DateTime<Utc>,
}

/// How often the Today view auto-refreshes calendar events, in minutes.
/// Settings key `gcal_refresh_minutes`; user-configurable in Settings.
pub const DEFAULT_REFRESH_MINUTES: u64 = 5;
/// Clamp bounds for the setting — a stray `0` would hammer the API on every
/// frame, and anything past a day isn't a "refresh rate" any more.
pub const MIN_REFRESH_MINUTES: u64 = 1;
pub const MAX_REFRESH_MINUTES: u64 = 1440;

/// The configured calendar auto-refresh interval in minutes, resolved from
/// the `settings` map the same "blank/garbage counts as absent" way as
/// [`GoogleCalendarConfig::resolve`]: an unset, unparsable, or out-of-range
/// value falls back to [`DEFAULT_REFRESH_MINUTES`], and anything in between
/// is clamped to `[MIN_REFRESH_MINUTES, MAX_REFRESH_MINUTES]`.
pub fn refresh_minutes(settings: &HashMap<String, String>) -> u64 {
    settings
        .get("gcal_refresh_minutes")
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&m| m >= MIN_REFRESH_MINUTES)
        .unwrap_or(DEFAULT_REFRESH_MINUTES)
        .min(MAX_REFRESH_MINUTES)
}

impl GoogleCalendarConfig {
    /// Settings keys: `gcal_client_id`, `gcal_client_secret`,
    /// `gcal_refresh_token`, `gcal_access_token`, `gcal_token_expiry` (an
    /// RFC3339 timestamp). `None` unless client id/secret/refresh token are
    /// all present and non-blank — a missing or unparsable access
    /// token/expiry just means the next fetch refreshes one before calling
    /// the API, same "blank counts as absent" convention as
    /// `SettingsDraft::load`.
    pub fn resolve(settings: &HashMap<String, String>) -> Option<Self> {
        let non_blank = |key: &str| {
            settings
                .get(key)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        Some(GoogleCalendarConfig {
            client_id: non_blank("gcal_client_id")?,
            client_secret: non_blank("gcal_client_secret")?,
            refresh_token: non_blank("gcal_refresh_token")?,
            access_token: non_blank("gcal_access_token").unwrap_or_default(),
            expiry: non_blank("gcal_token_expiry")
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or(DateTime::<Utc>::MIN_UTC),
        })
    }

    /// A minute of slack before the real expiry, so a token that's about to
    /// expire mid-request gets refreshed proactively instead of failing.
    fn is_expired(&self) -> bool {
        Utc::now() + Duration::minutes(1) >= self.expiry
    }
}

/// Spawns a background thread that (refreshing the access token first if
/// it's expired or missing) fetches `today`'s events from the connected
/// Google Calendar. Mirrors `sync::run_async`: opens its own connection to
/// `db_path` so a refreshed token can be persisted from the background
/// thread even while the UI thread's connection is also live, delivering
/// the result over the returned channel for the caller to `try_recv` once
/// per frame.
pub fn run_async(
    db_path: String,
    config: GoogleCalendarConfig,
    today: NaiveDate,
) -> Receiver<Result<Vec<CalendarEvent>, String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = fetch(db_path, config, today);
        let _ = tx.send(result);
    });
    rx
}

fn fetch(
    db_path: String,
    mut config: GoogleCalendarConfig,
    today: NaiveDate,
) -> Result<Vec<CalendarEvent>, String> {
    if config.is_expired() {
        let (access_token, expiry) = oauth::refresh_access_token(
            &config.client_id,
            &config.client_secret,
            &config.refresh_token,
        )?;
        config.access_token = access_token;
        config.expiry = expiry;

        let conn = crate::db::open(&db_path).map_err(|e| e.to_string())?;
        settings_repo::set_many(
            &conn,
            &[
                ("gcal_access_token", config.access_token.as_str()),
                ("gcal_token_expiry", &config.expiry.to_rfc3339()),
            ],
        )
        .map_err(|e| e.to_string())?;
    }

    google::fetch_today_events(&config.access_token, today)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn resolves_when_fully_connected() {
        let settings = settings(&[
            ("gcal_client_id", "id"),
            ("gcal_client_secret", "secret"),
            ("gcal_refresh_token", "refresh"),
            ("gcal_access_token", "access"),
            ("gcal_token_expiry", "2026-08-22T12:00:00Z"),
        ]);
        let config = GoogleCalendarConfig::resolve(&settings).unwrap();
        assert_eq!(config.client_id, "id");
        assert_eq!(config.client_secret, "secret");
        assert_eq!(config.refresh_token, "refresh");
        assert_eq!(config.access_token, "access");
    }

    #[test]
    fn refresh_minutes_falls_back_to_the_default_when_unset_or_unusable() {
        assert_eq!(refresh_minutes(&HashMap::new()), DEFAULT_REFRESH_MINUTES);
        assert_eq!(
            refresh_minutes(&settings(&[("gcal_refresh_minutes", "banana")])),
            DEFAULT_REFRESH_MINUTES
        );
        assert_eq!(
            refresh_minutes(&settings(&[("gcal_refresh_minutes", "0")])),
            DEFAULT_REFRESH_MINUTES
        );
    }

    #[test]
    fn refresh_minutes_reads_a_valid_value_and_clamps_a_huge_one() {
        assert_eq!(
            refresh_minutes(&settings(&[("gcal_refresh_minutes", "15")])),
            15
        );
        assert_eq!(
            refresh_minutes(&settings(&[("gcal_refresh_minutes", "999999")])),
            MAX_REFRESH_MINUTES
        );
    }

    #[test]
    fn none_when_never_connected() {
        assert!(GoogleCalendarConfig::resolve(&HashMap::new()).is_none());
    }

    #[test]
    fn none_when_refresh_token_is_missing() {
        let settings = settings(&[("gcal_client_id", "id"), ("gcal_client_secret", "secret")]);
        assert!(GoogleCalendarConfig::resolve(&settings).is_none());
    }

    #[test]
    fn blank_refresh_token_counts_as_absent() {
        let settings = settings(&[
            ("gcal_client_id", "id"),
            ("gcal_client_secret", "secret"),
            ("gcal_refresh_token", "   "),
        ]);
        assert!(GoogleCalendarConfig::resolve(&settings).is_none());
    }

    #[test]
    fn missing_access_token_and_expiry_still_resolves() {
        let settings = settings(&[
            ("gcal_client_id", "id"),
            ("gcal_client_secret", "secret"),
            ("gcal_refresh_token", "refresh"),
        ]);
        let config = GoogleCalendarConfig::resolve(&settings).unwrap();
        assert_eq!(config.access_token, "");
        assert!(config.is_expired());
    }
}
