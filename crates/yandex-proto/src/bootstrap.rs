use reqwest::header::{ACCEPT, COOKIE, HeaderValue, REFERER};
use reqwest::multipart::Form;
use serde_json::Value;
use std::time::{Duration, Instant};
use tracing::info;

use crate::net::{describe, failed, millis, redact_url};
use crate::session::{Identity, SessionCookie, cookie_header};

const REFERER_URL: &str = "https://yandex.ru/chat";

/// Registry error codes that mean the cookies are stale or missing.
const COOKIE_ERROR_CODES: [&str; 4] = [
    "invalid_cookies",
    "not_authorized",
    "no_credentials",
    "cookie_auth_failed",
];

#[derive(Clone, Debug)]
pub struct RegistryEndpoints {
    pub api_url: String,
    pub csrf_url: String,
    /// Total deadline per HTTP request; a silent server must not hang sign-in.
    pub timeout: Duration,
}

impl RegistryEndpoints {
    pub fn production() -> Self {
        Self {
            api_url: "https://yandex.ru/messenger/api/registry/api/".into(),
            csrf_url: "https://yandex.ru/messenger/api/registry/csrf-token/".into(),
            timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("the server rejected the session cookies")]
    SessionRejected,
    #[error("the server rejected the CSRF token")]
    BadCsrf,
    #[error("unexpected server response: {0}")]
    Protocol(String),
    #[error("network error: {0}")]
    Network(String),
}

impl From<reqwest::Error> for BootstrapError {
    fn from(e: reqwest::Error) -> Self {
        Self::Network(describe(&e.without_url()))
    }
}

/// Fetches a CSRF token, then asks `request_user` who the cookies belong to.
/// Read-only: neither call changes server state.
pub async fn bootstrap(
    endpoints: &RegistryEndpoints,
    cookies: &[SessionCookie],
) -> Result<Identity, BootstrapError> {
    let host = reqwest::Url::parse(&endpoints.api_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .ok_or_else(|| BootstrapError::Protocol("invalid api url".into()))?;
    let mut cookie = HeaderValue::from_str(&cookie_header(cookies, &host))
        .map_err(|_| BootstrapError::Protocol("cookie value is not a valid header".into()))?;
    cookie.set_sensitive(true);

    let client = reqwest::Client::builder()
        .connect_timeout(endpoints.timeout)
        .timeout(endpoints.timeout)
        .build()?;
    let common = |req: reqwest::RequestBuilder| {
        req.header(COOKIE, cookie.clone())
            .header(ACCEPT, "application/json")
            .header(REFERER, REFERER_URL)
    };

    // Steps log status only: bodies carry the CSRF token and user data.
    let started = Instant::now();
    let fail = |started| failed("csrf-token", "POST", &endpoints.csrf_url, started);
    let response = common(client.post(&endpoints.csrf_url))
        .send()
        .await
        .map_err(fail(started))?;
    let status = response.status().as_u16();
    let body = response.bytes().await.map_err(fail(started))?;
    info!(step = "csrf-token", url = %redact_url(&endpoints.csrf_url), status, elapsed_ms = millis(started.elapsed()), "bootstrap");
    let token = parse_csrf_response(status, &body)?;

    let form = Form::new().text(
        "request",
        r#"{"method":"request_user","params":{"bind_phone_number":false}}"#,
    );
    let started = Instant::now();
    let fail = |started| failed("request_user", "POST", &endpoints.api_url, started);
    let response = common(client.post(&endpoints.api_url))
        .header("X-CSRF-TOKEN", token)
        .multipart(form)
        .send()
        .await
        .map_err(fail(started))?;
    let status = response.status().as_u16();
    let body = response.bytes().await.map_err(fail(started))?;
    info!(step = "request_user", url = %redact_url(&endpoints.api_url), status, elapsed_ms = millis(started.elapsed()), "bootstrap");
    parse_request_user_response(status, &body)
}

fn parse_csrf_response(status: u16, body: &[u8]) -> Result<String, BootstrapError> {
    match status {
        401 | 403 => return Err(BootstrapError::SessionRejected),
        200..=299 => {}
        _ => {
            return Err(BootstrapError::Protocol(format!(
                "csrf-token returned HTTP {status}"
            )));
        }
    }
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("token")?.as_str().map(str::to_owned))
        .filter(|t| !t.is_empty())
        .ok_or_else(|| BootstrapError::Protocol("csrf-token response has no token".into()))
}

fn parse_request_user_response(status: u16, body: &[u8]) -> Result<Identity, BootstrapError> {
    if status == 401 {
        return Err(BootstrapError::SessionRejected);
    }
    let payload: Value = serde_json::from_slice(body).map_err(|_| {
        BootstrapError::Protocol(format!(
            "request_user returned HTTP {status} with a non-JSON body"
        ))
    })?;
    let data = payload.get("data");

    if payload.get("status").and_then(Value::as_str) != Some("ok") {
        let code = data
            .and_then(|d| d.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return Err(if COOKIE_ERROR_CODES.contains(&code) {
            BootstrapError::SessionRejected
        } else if code == "bad_csrf_token" {
            BootstrapError::BadCsrf
        } else {
            BootstrapError::Protocol(format!("request_user failed: {code}"))
        });
    }

    let data =
        data.ok_or_else(|| BootstrapError::Protocol("request_user response has no data".into()))?;
    let Some(user) = data.get("user") else {
        // Without a `user` object the server answered as for an anonymous guest.
        return Err(if data.get("guid").is_some() {
            BootstrapError::SessionRejected
        } else {
            BootstrapError::Protocol("request_user response has no user".into())
        });
    };
    let guid = user
        .get("guid")
        .and_then(Value::as_str)
        .ok_or_else(|| BootstrapError::Protocol("request_user response has no guid".into()))?;
    let uid = match user.get("uid") {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    let display_name = user
        .get("display_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Ok(Identity {
        uid,
        guid: guid.to_owned(),
        display_name,
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};

    use super::*;

    fn ok_body(user: &str) -> String {
        format!(r#"{{"status":"ok","data":{{"user":{user}}}}}"#)
    }

    fn err_body(code: &str) -> String {
        format!(r#"{{"status":"error","data":{{"code":"{code}"}}}}"#)
    }

    fn parse(status: u16, body: &str) -> Result<Identity, BootstrapError> {
        parse_request_user_response(status, body.as_bytes())
    }

    #[test]
    fn parses_numeric_and_string_uid() {
        let id = parse(
            200,
            &ok_body(r#"{"guid":"g1","uid":4001,"display_name":"Ivan"}"#),
        )
        .unwrap();
        assert_eq!(
            id,
            Identity {
                uid: Some("4001".into()),
                guid: "g1".into(),
                display_name: Some("Ivan".into())
            }
        );
        let id = parse(200, &ok_body(r#"{"guid":"g2","uid":"77"}"#)).unwrap();
        assert_eq!(id.uid.as_deref(), Some("77"));
        assert_eq!(id.display_name, None);
        let id = parse(200, &ok_body(r#"{"guid":"g3"}"#)).unwrap();
        assert_eq!(id.uid, None);
        assert_eq!(id.xiva_user(), "g3");
    }

    #[test]
    fn maps_cookie_rejection() {
        assert!(matches!(
            parse(401, ""),
            Err(BootstrapError::SessionRejected)
        ));
        for code in COOKIE_ERROR_CODES {
            assert!(
                matches!(
                    parse(403, &err_body(code)),
                    Err(BootstrapError::SessionRejected)
                ),
                "{code}"
            );
        }
    }

    #[test]
    fn guest_answer_is_rejection() {
        let body = r#"{"status":"ok","data":{"guid":"guest"}}"#;
        assert!(matches!(
            parse(200, body),
            Err(BootstrapError::SessionRejected)
        ));
    }

    #[test]
    fn maps_bad_csrf() {
        assert!(matches!(
            parse(403, &err_body("bad_csrf_token")),
            Err(BootstrapError::BadCsrf)
        ));
    }

    #[test]
    fn maps_protocol_errors() {
        assert!(matches!(
            parse(200, &ok_body(r#"{"uid":1}"#)),
            Err(BootstrapError::Protocol(_))
        ));
        assert!(matches!(
            parse(500, &err_body("internal")),
            Err(BootstrapError::Protocol(_))
        ));
        assert!(matches!(
            parse(500, r#"{"status":"error"}"#),
            Err(BootstrapError::Protocol(_))
        ));
        assert!(matches!(
            parse(502, "<html>"),
            Err(BootstrapError::Protocol(_))
        ));
        assert!(matches!(
            parse(200, r#"{"status":"ok"}"#),
            Err(BootstrapError::Protocol(_))
        ));
    }

    #[test]
    fn csrf_response_mapping() {
        assert_eq!(
            parse_csrf_response(200, br#"{"token":"t1"}"#).unwrap(),
            "t1"
        );
        assert!(matches!(
            parse_csrf_response(401, b""),
            Err(BootstrapError::SessionRejected)
        ));
        assert!(matches!(
            parse_csrf_response(403, b""),
            Err(BootstrapError::SessionRejected)
        ));
        assert!(matches!(
            parse_csrf_response(500, b""),
            Err(BootstrapError::Protocol(_))
        ));
        assert!(matches!(
            parse_csrf_response(200, b"{}"),
            Err(BootstrapError::Protocol(_))
        ));
        assert!(matches!(
            parse_csrf_response(200, br#"{"token":""}"#),
            Err(BootstrapError::Protocol(_))
        ));
    }

    /// Serves one canned `(status, body)` per incoming connection, then returns
    /// the raw requests it saw.
    fn stub(responses: Vec<(u16, String)>) -> (RegistryEndpoints, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let endpoints = RegistryEndpoints {
            api_url: format!("{base}/api/"),
            csrf_url: format!("{base}/csrf-token/"),
            timeout: Duration::from_secs(5),
        };
        let handle = thread::spawn(move || {
            let mut seen = Vec::new();
            for (status, body) in responses {
                let (mut conn, _) = listener.accept().unwrap();
                seen.push(read_request(&mut conn));
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                conn.write_all(reply.as_bytes()).unwrap();
            }
            seen
        });
        (endpoints, handle)
    }

    fn read_request(conn: &mut impl Read) -> String {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = conn.read(&mut chunk).unwrap();
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf).into_owned();
            if let Some(head_end) = text.find("\r\n\r\n") {
                let head = text[..head_end].to_ascii_lowercase();
                let body = &text[head_end + 4..];
                let done = match head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .map(|v| v.trim().parse::<usize>().unwrap())
                {
                    Some(len) => body.len() >= len,
                    None if head.contains("transfer-encoding: chunked") => {
                        body.ends_with("0\r\n\r\n")
                    }
                    None => true,
                };
                if done {
                    return text;
                }
            }
            assert!(n > 0, "connection closed before request completed");
        }
    }

    fn local_cookies() -> Vec<SessionCookie> {
        vec![SessionCookie {
            name: "Session_id".into(),
            value: "SECRET".into(),
            domain: "127.0.0.1".into(),
            path: "/".into(),
            expires_unix: None,
            secure: true,
            http_only: true,
        }]
    }

    #[tokio::test]
    async fn bootstrap_success_sends_expected_requests() {
        let (endpoints, server) = stub(vec![
            (200, r#"{"token":"tok-1"}"#.into()),
            (
                200,
                ok_body(r#"{"guid":"g1","uid":4001,"display_name":"Ivan"}"#),
            ),
        ]);
        let id = bootstrap(&endpoints, &local_cookies()).await.unwrap();
        assert_eq!(id.uid.as_deref(), Some("4001"));
        assert_eq!(id.guid, "g1");
        assert_eq!(id.display_name.as_deref(), Some("Ivan"));

        let seen = server.join().unwrap();
        let (csrf, api) = (seen[0].to_ascii_lowercase(), seen[1].to_ascii_lowercase());
        assert!(csrf.starts_with("post /csrf-token/ "));
        assert!(api.starts_with("post /api/ "));
        for req in [&csrf, &api] {
            assert!(req.contains("cookie: session_id=secret"));
            assert!(req.contains("referer: https://yandex.ru/chat"));
        }
        assert!(api.contains("x-csrf-token: tok-1"));
        assert!(api.contains("multipart/form-data"));
        assert!(api.contains(r#"name="request""#));
        assert!(api.contains(r#"{"method":"request_user","params":{"bind_phone_number":false}}"#));
    }

    #[tokio::test]
    async fn bootstrap_maps_csrf_endpoint_rejection() {
        let (endpoints, server) = stub(vec![(401, String::new())]);
        let err = bootstrap(&endpoints, &local_cookies()).await.unwrap_err();
        assert!(matches!(err, BootstrapError::SessionRejected));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn bootstrap_maps_api_errors() {
        let (endpoints, server) = stub(vec![
            (200, r#"{"token":"t"}"#.into()),
            (403, err_body("bad_csrf_token")),
        ]);
        let err = bootstrap(&endpoints, &local_cookies()).await.unwrap_err();
        assert!(matches!(err, BootstrapError::BadCsrf));
        server.join().unwrap();

        let (endpoints, server) = stub(vec![
            (200, r#"{"token":"t"}"#.into()),
            (200, err_body("invalid_cookies")),
        ]);
        let err = bootstrap(&endpoints, &local_cookies()).await.unwrap_err();
        assert!(matches!(err, BootstrapError::SessionRejected));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn bootstrap_maps_transport_failure() {
        // Bind then drop to get a port with nothing listening.
        let addr = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let endpoints = RegistryEndpoints {
            api_url: format!("http://{addr}/api/"),
            csrf_url: format!("http://{addr}/csrf-token/"),
            timeout: Duration::from_secs(5),
        };
        let err = bootstrap(&endpoints, &local_cookies()).await.unwrap_err();
        assert!(matches!(err, BootstrapError::Network(_)));
    }

    #[tokio::test]
    async fn bootstrap_times_out_on_silent_server() {
        // Accepts the connection but never answers.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let endpoints = RegistryEndpoints {
            api_url: format!("http://{addr}/api/"),
            csrf_url: format!("http://{addr}/csrf-token/"),
            timeout: Duration::from_millis(300),
        };
        let capture = crate::test_log::Capture::default();
        let _guard = capture.install();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            bootstrap(&endpoints, &local_cookies()),
        )
        .await;
        assert!(matches!(result, Ok(Err(BootstrapError::Network(_)))));
        let text = capture.text();
        let line = text
            .lines()
            .find(|l| l.contains("http exchange failed"))
            .unwrap_or_else(|| panic!("no failure line:\n{text}"));
        for needle in [
            "WARN",
            "call=\"csrf-token\"",
            "method=\"POST\"",
            "/csrf-token/",
            "elapsed_ms=",
            "timeout=true",
        ] {
            assert!(line.contains(needle), "{needle}: {line}");
        }
        drop(listener);
    }

    #[tokio::test]
    async fn log_shows_steps_but_no_secrets_or_bodies() {
        use crate::test_log::Capture;

        let capture = Capture::default();
        let _guard = capture.install();
        let (mut endpoints, server) = stub(vec![
            (200, r#"{"token":"tok-CSRF-1"}"#.into()),
            (
                200,
                ok_body(r#"{"guid":"g-body","uid":4001,"display_name":"Ivan-BODY"}"#),
            ),
        ]);
        endpoints.csrf_url.push_str("?uid=Q-VALUE-1");
        endpoints.api_url.push_str("?uid=Q-VALUE-2");
        bootstrap(&endpoints, &local_cookies()).await.unwrap();
        server.join().unwrap();

        let text = capture.text();
        for needle in [
            "csrf-token",
            "request_user",
            "127.0.0.1/csrf-token/",
            "127.0.0.1/api/",
            "status=200",
        ] {
            assert!(text.contains(needle), "{needle}: {text}");
        }
        capture.assert_absent(&[
            "SECRET",
            "tok-CSRF-1",
            "Q-VALUE-1",
            "Q-VALUE-2",
            "g-body",
            "Ivan-BODY",
        ]);
    }
}
