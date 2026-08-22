use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use uuid::Uuid;

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";
/// How long `connect_async` waits for Google's redirect before giving up —
/// covers the browser tab being closed or abandoned mid-flow, which would
/// otherwise leave `listener.accept()` blocked forever.
const REDIRECT_WAIT_TIMEOUT: StdDuration = StdDuration::from_secs(300);

pub struct GoogleTokenSet {
    pub access_token: String,
    pub refresh_token: String,
    pub expiry: DateTime<Utc>,
}

/// Spawns a background thread that runs the full loopback OAuth flow: opens
/// a local port, launches the system browser to Google's consent screen,
/// waits for the redirect back, and exchanges the resulting code for
/// tokens. Delivered over the returned channel, polled once per frame —
/// same shape as `sync::run_async`/`llm::parse_capture_async`.
pub fn connect_async(
    client_id: String,
    client_secret: String,
) -> Receiver<Result<GoogleTokenSet, String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = run_oauth_flow(&client_id, &client_secret);
        let _ = tx.send(result);
    });
    rx
}

fn run_oauth_flow(client_id: &str, client_secret: &str) -> Result<GoogleTokenSet, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("couldn't open a local port for Google's sign-in redirect: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/");
    let csrf_state = Uuid::new_v4().to_string();

    let auth_url = build_auth_url(client_id, &redirect_uri, &csrf_state);
    webbrowser::open(&auth_url).map_err(|e| format!("couldn't open your browser: {e}"))?;

    let (code, returned_state) = accept_redirect(&listener)?;
    if returned_state != csrf_state {
        return Err(
            "Google's sign-in response didn't match this request \u{2014} try connecting again"
                .to_string(),
        );
    }

    exchange_code(client_id, client_secret, &code, &redirect_uri)
}

/// Forces `prompt=consent` on every connect: Google only returns a
/// `refresh_token` on a fresh consent grant, and without this a reconnect
/// (after a disconnect, or a grant revoked on Google's side) would silently
/// fail to get one back.
fn build_auth_url(client_id: &str, redirect_uri: &str, state: &str) -> String {
    let query: String = form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("response_type", "code"),
            ("scope", SCOPE),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("state", state),
        ])
        .finish();
    format!("https://accounts.google.com/o/oauth2/v2/auth?{query}")
}

/// Blocks (via short-sleep polling, not a hard block) until Google's
/// redirect hits the local listener, or `REDIRECT_WAIT_TIMEOUT` passes with
/// the browser tab apparently abandoned.
fn accept_redirect(listener: &TcpListener) -> Result<(String, String), String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + REDIRECT_WAIT_TIMEOUT;
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(
                        "timed out waiting for you to sign in \u{2014} try connecting again"
                            .to_string(),
                    );
                }
                thread::sleep(StdDuration::from_millis(200));
            }
            Err(e) => return Err(format!("failed to accept Google's redirect: {e}")),
        }
    };
    stream.set_nonblocking(false).map_err(|e| e.to_string())?;

    let mut request_line = String::new();
    BufReader::new(&stream)
        .read_line(&mut request_line)
        .map_err(|e| format!("failed to read Google's redirect: {e}"))?;

    let result = extract_code_and_state(&request_line);

    let body =
        "<html><body>Signed in \u{2014} you can close this tab and return to Wu Wei.</body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());

    result
}

/// Pulls `code`/`state` out of a raw HTTP request line
/// (`GET /?code=...&state=... HTTP/1.1`), percent-decoding along the way —
/// an authorization code can contain characters like `/` that arrive
/// percent-encoded. Also surfaces `error=` (e.g. the user declining consent)
/// as a readable message instead of a missing-code error.
fn extract_code_and_state(request_line: &str) -> Result<(String, String), String> {
    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "Google's redirect wasn't a valid HTTP request".to_string())?;
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    let params: HashMap<String, String> = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();

    if let Some(err) = params.get("error") {
        return Err(format!("Google sign-in didn't complete ({err})"));
    }
    let code = params
        .get("code")
        .cloned()
        .ok_or_else(|| "Google's redirect didn't include an authorization code".to_string())?;
    let state = params.get("state").cloned().unwrap_or_default();
    Ok((code, state))
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: i64,
}

fn exchange_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<GoogleTokenSet, String> {
    let mut response = ureq::post(TOKEN_URL)
        .send_form([
            ("code", code),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .map_err(|e| format!("Google token exchange failed: {e}"))?;

    let body: TokenResponse = response
        .body_mut()
        .read_json()
        .map_err(|e| format!("failed to read Google's token response: {e}"))?;

    let refresh_token = body.refresh_token.ok_or_else(|| {
        "Google didn't return a refresh token \u{2014} try disconnecting Wu Wei at \
         https://myaccount.google.com/permissions and connecting again"
            .to_string()
    })?;

    Ok(GoogleTokenSet {
        access_token: body.access_token,
        refresh_token,
        expiry: Utc::now() + Duration::seconds(body.expires_in),
    })
}

/// Exchanges a refresh token for a new access token — same wire format as
/// `exchange_code`, just `grant_type=refresh_token`. Called from
/// `calendar::fetch` on a background thread whenever the cached access
/// token has expired.
pub fn refresh_access_token(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<(String, DateTime<Utc>), String> {
    let mut response = ureq::post(TOKEN_URL)
        .send_form([
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .map_err(|e| format!("Google token refresh failed: {e}"))?;

    let body: TokenResponse = response
        .body_mut()
        .read_json()
        .map_err(|e| format!("failed to read Google's token response: {e}"))?;

    Ok((
        body.access_token,
        Utc::now() + Duration::seconds(body.expires_in),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_code_and_state_from_a_request_line() {
        let line = "GET /?code=4%2F0Ab_test&state=abc123 HTTP/1.1\r\n";
        let (code, state) = extract_code_and_state(line).unwrap();
        assert_eq!(code, "4/0Ab_test");
        assert_eq!(state, "abc123");
    }

    #[test]
    fn reports_denied_consent() {
        let line = "GET /?error=access_denied&state=abc123 HTTP/1.1\r\n";
        let err = extract_code_and_state(line).unwrap_err();
        assert!(err.contains("access_denied"));
    }

    #[test]
    fn errors_when_no_code_is_present() {
        let line = "GET / HTTP/1.1\r\n";
        assert!(extract_code_and_state(line).is_err());
    }

    #[test]
    fn auth_url_forces_consent_and_offline_access() {
        let url = build_auth_url("client-123", "http://127.0.0.1:9999/", "state-abc");
        assert!(url.contains("prompt=consent"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("client_id=client-123"));
        assert!(url.contains("state=state-abc"));
    }
}
