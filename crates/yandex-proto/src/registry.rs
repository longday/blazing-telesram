//! Registry HTTP reads (`request_user`, `get_users_data`), reusing the bootstrap's wire format.

use std::time::Instant;

use reqwest::header::{ACCEPT, COOKIE, HeaderValue, REFERER};
use reqwest::multipart::Form;
use serde_json::Value as Json;
use tracing::{debug, trace};

use crate::bootstrap::RegistryEndpoints;
use crate::error::ProtoError;
use crate::net::{HttpSettings, cookie_value, failed, host_of, http_client, millis, redact_url};
use crate::request::{ReadRequest, Route, SearchEntity};
use crate::search::{SearchReply, parse_search};
use crate::session::SessionCookie;
use crate::wire::{WireUser, parse_users};

const REFERER_URL: &str = "https://yandex.ru/chat";

/// Registry error codes that mean the cookies are stale or missing.
const COOKIE_ERROR_CODES: [&str; 4] = ["invalid_cookies", "not_authorized", "no_credentials", "cookie_auth_failed"];

#[derive(Clone)]
pub struct RegistryClient {
    http: reqwest::Client,
    api_url: String,
    csrf_url: String,
    cookie: HeaderValue,
}

impl RegistryClient {
    pub fn new(endpoints: &RegistryEndpoints, settings: &HttpSettings, cookies: &[SessionCookie]) -> Result<Self, ProtoError> {
        Ok(Self {
            http: http_client(settings)?,
            cookie: cookie_value(cookies, &host_of(&endpoints.api_url)?)?,
            api_url: endpoints.api_url.clone(),
            csrf_url: endpoints.csrf_url.clone(),
        })
    }

    fn common(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.header(COOKIE, self.cookie.clone()).header(ACCEPT, "application/json").header(REFERER, REFERER_URL)
    }

    async fn csrf_token(&self) -> Result<String, ProtoError> {
        let started = Instant::now();
        let fail = |started| failed("csrf-token", "POST", &self.csrf_url, started);
        let response = self.common(self.http.post(&self.csrf_url)).send().await.map_err(fail(started))?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(fail(started))?;
        // The reply body carries the token: it is never logged.
        debug!(method = "POST", url = %redact_url(&self.csrf_url), status, elapsed_ms = millis(started.elapsed()), "csrf-token exchange");
        match status {
            401 | 403 => return Err(ProtoError::SessionExpired),
            200..=299 => {}
            _ => return Err(http_error("csrf-token", status)),
        }
        serde_json::from_slice::<Json>(&body)
            .ok()
            .and_then(|v| v.get("token")?.as_str().map(str::to_owned))
            .filter(|t| !t.is_empty())
            .ok_or_else(|| ProtoError::Protocol("csrf-token response has no token".into()))
    }

    /// Runs a registry-routed request and returns its `data` object.
    pub(crate) async fn execute(&self, request: &ReadRequest) -> Result<Json, ProtoError> {
        let (Route::Registry { csrf }, Some(body)) = (request.route(), request.registry_body()) else {
            return Err(ProtoError::Protocol("not a registry request".into()));
        };
        let mut call = self.common(self.http.post(&self.api_url)).multipart(Form::new().text("request", body));
        if csrf {
            call = call.header("X-CSRF-TOKEN", self.csrf_token().await?);
        }
        let started = Instant::now();
        let fail = |started| failed(request.method(), "POST", &self.api_url, started);
        let response = call.send().await.map_err(fail(started))?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(fail(started))?;
        debug!(
            method = "POST",
            url = %redact_url(&self.api_url),
            call = request.method(),
            status,
            elapsed_ms = millis(started.elapsed()),
            "registry exchange"
        );
        trace!(call = request.method(), body = %String::from_utf8_lossy(&body), "registry reply body");
        if status == 401 {
            return Err(ProtoError::SessionExpired);
        }
        let payload: Json = serde_json::from_slice(&body)
            .map_err(|_| ProtoError::Protocol(format!("{} returned HTTP {status} with a non-JSON body", request.method())))?;
        if payload.get("status").and_then(Json::as_str) != Some("ok") {
            let code = payload.pointer("/data/code").and_then(Json::as_str).unwrap_or("unknown");
            return Err(if COOKIE_ERROR_CODES.contains(&code) {
                ProtoError::SessionExpired
            } else if status >= 500 {
                http_error(request.method(), status)
            } else {
                ProtoError::Protocol(format!("{} failed: {code}", request.method()))
            });
        }
        payload.get("data").cloned().ok_or_else(|| ProtoError::Protocol(format!("{} reply has no data", request.method())))
    }

    /// Display names and avatars for `guids` (batched by the caller).
    pub async fn users(&self, guids: &[String]) -> Result<Vec<WireUser>, ProtoError> {
        if guids.is_empty() {
            return Ok(Vec::new());
        }
        parse_users(&self.execute(&ReadRequest::GetUsersData { guids: guids.to_vec() }).await?)
    }

    /// One `search` for `query`: a result set per requested entity (first page only). The query
    /// text goes on the wire and into the `trace` log only.
    pub async fn search(&self, query: &str, limit: u32, entities: &[SearchEntity]) -> Result<SearchReply, ProtoError> {
        let request = ReadRequest::Search { query: query.to_owned(), limit, entities: entities.to_vec() };
        parse_search(&self.execute(&request).await?, entities)
    }
}

/// 5xx and throttling clear by themselves; other statuses are protocol surprises.
pub(crate) fn http_error(what: &str, status: u16) -> ProtoError {
    if status == 429 || status >= 500 {
        ProtoError::Network(format!("{what} returned HTTP {status}"))
    } else {
        ProtoError::Protocol(format!("{what} returned HTTP {status}"))
    }
}

#[cfg(test)]
pub(crate) mod stub {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    /// One-shot-per-connection HTTP stub: replies with `responses[i]` to the i-th request and
    /// records the raw requests. `None` makes the connection hang silently.
    pub fn serve(responses: Vec<Option<String>>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = vec![0u8; 16384];
                let n = stream.read(&mut buf).unwrap_or(0);
                log.lock().unwrap().push(String::from_utf8_lossy(&buf[..n]).into_owned());
                match response {
                    Some(r) => {
                        let _ = stream.write_all(r.as_bytes());
                    }
                    None => thread::sleep(Duration::from_secs(3)),
                }
            }
        });
        (base, seen)
    }

    pub fn json_response(status: &str, body: &str) -> Option<String> {
        Some(format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::stub::{json_response, serve};
    use super::*;

    fn cookie() -> SessionCookie {
        SessionCookie {
            name: "Session_id".into(),
            value: "secret".into(),
            domain: ".yandex.ru".into(),
            path: "/".into(),
            expires_unix: None,
            secure: true,
            http_only: true,
        }
    }

    fn client(base: &str, timeout: Duration) -> RegistryClient {
        let mut endpoints = RegistryEndpoints::production();
        endpoints.api_url = format!("{base}/api/");
        endpoints.csrf_url = format!("{base}/csrf/");
        let settings = HttpSettings { connect_timeout: timeout, request_timeout: timeout };
        RegistryClient::new(&endpoints, &settings, &[cookie()]).unwrap()
    }

    #[tokio::test]
    async fn users_are_read_without_csrf() {
        let (base, seen) = serve(vec![json_response(
            "200 OK",
            r#"{"status":"ok","data":{"users":[{"guid":"g1","display_name":"Ivan","avatar_id":"a1"}]}}"#,
        )]);
        let users = client(&base, Duration::from_secs(5)).users(&["g1".into()]).await.unwrap();
        assert_eq!(users[0].display_name.as_deref(), Some("Ivan"));
        let request = seen.lock().unwrap()[0].to_lowercase();
        assert!(request.starts_with("post /api/"));
        assert!(request.contains("get_users_data") && request.contains("accept: application/json"));
        assert!(!request.contains("x-csrf-token"));
        assert!(client(&base, Duration::from_secs(5)).users(&[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn request_user_fetches_a_csrf_token_first() {
        let (base, seen) = serve(vec![
            json_response("200 OK", r#"{"token":"t0k"}"#),
            json_response("200 OK", r#"{"status":"ok","data":{"user":{"guid":"g"}}}"#),
        ]);
        let data = client(&base, Duration::from_secs(5)).execute(&ReadRequest::RequestUser).await.unwrap();
        assert_eq!(data["user"]["guid"], "g");
        let seen = seen.lock().unwrap();
        assert!(seen[0].starts_with("POST /csrf/"));
        assert!(seen[1].to_lowercase().contains("x-csrf-token: t0k"));
    }

    #[tokio::test]
    async fn cookie_errors_and_401_expire_the_session() {
        let (base, _) = serve(vec![
            json_response("200 OK", r#"{"status":"error","data":{"code":"invalid_cookies"}}"#),
            json_response("401 Unauthorized", "{}"),
            json_response("200 OK", r#"{"status":"error","data":{"code":"weird"}}"#),
            json_response("503 Service Unavailable", "{}"),
        ]);
        let c = client(&base, Duration::from_secs(5));
        let guids = ["g".to_owned()];
        let ask = || c.users(&guids);
        assert_eq!(ask().await.unwrap_err(), ProtoError::SessionExpired);
        assert_eq!(ask().await.unwrap_err(), ProtoError::SessionExpired);
        assert!(matches!(ask().await.unwrap_err(), ProtoError::Protocol(m) if m.contains("weird")));
        assert!(matches!(ask().await.unwrap_err(), ProtoError::Network(_)));
    }

    #[tokio::test]
    async fn silent_server_times_out_as_a_network_error() {
        let (base, _) = serve(vec![None]);
        let err = client(&base, Duration::from_millis(200)).users(&["g".into()]).await.unwrap_err();
        assert!(matches!(err, ProtoError::Network(_)));
    }

    #[tokio::test]
    async fn socket_requests_are_not_executed_over_http() {
        let c = client("http://127.0.0.1:1", Duration::from_secs(1));
        assert!(matches!(c.execute(&ReadRequest::ChatList).await, Err(ProtoError::Protocol(_))));
    }

    const SET: &str = r#"{"items":[{"x":1}],"total":1,"limit":10,"page":1,"pages":1}"#;
    const EMPTY_SET: &str = r#"{"items":[],"total":0,"limit":10,"page":1,"pages":1}"#;
    const ALL: [SearchEntity; 3] = [SearchEntity::Messages, SearchEntity::Users, SearchEntity::Chats];

    #[tokio::test]
    async fn search_posts_the_query_without_csrf_and_parses_the_sets() {
        let reply = format!(r#"{{"status":"ok","data":{{"messages":{SET},"users":{EMPTY_SET},"chats":{EMPTY_SET}}}}}"#);
        let (base, seen) = serve(vec![json_response("200 OK", &reply)]);
        let found = client(&base, Duration::from_secs(5)).search("needle", 10, &ALL).await.unwrap();
        assert_eq!(found.messages.total, 1);
        assert_eq!(found.messages.items.len(), 1);
        assert!(found.users.items.is_empty() && found.chats.items.is_empty());
        let request = seen.lock().unwrap()[0].to_lowercase();
        assert!(request.starts_with("post /api/") && !request.contains("x-csrf-token"));
        assert!(request.contains(r#""method":"search""#) && request.contains("needle"), "{request}");
        assert!(request.contains(r#""entities":["messages","users","chats"]"#), "{request}");
    }

    #[tokio::test]
    async fn search_with_no_matches_returns_empty_sets() {
        let reply = format!(r#"{{"status":"ok","data":{{"messages":{EMPTY_SET},"users":{EMPTY_SET},"chats":{EMPTY_SET}}}}}"#);
        let (base, _) = serve(vec![json_response("200 OK", &reply)]);
        let found = client(&base, Duration::from_secs(5)).search("nothing", 10, &ALL).await.unwrap();
        for set in [&found.messages, &found.users, &found.chats] {
            assert!(set.items.is_empty());
            assert_eq!((set.total, set.limit, set.page, set.pages), (0, 10, 1, 1));
        }
    }

    #[tokio::test]
    async fn search_reply_with_a_missing_set_or_no_data_is_a_protocol_error() {
        let (base, _) = serve(vec![
            json_response("200 OK", &format!(r#"{{"status":"ok","data":{{"messages":{SET},"users":{SET}}}}}"#)),
            json_response("200 OK", r#"{"status":"ok"}"#),
        ]);
        let c = client(&base, Duration::from_secs(5));
        assert!(matches!(c.search("q", 10, &ALL).await.unwrap_err(), ProtoError::Protocol(m) if m.contains("chats")));
        assert!(matches!(c.search("q", 10, &ALL).await.unwrap_err(), ProtoError::Protocol(m) if m.contains("no data")));
    }

    #[tokio::test]
    async fn search_cookie_error_maps_to_session_expired() {
        let (base, _) = serve(vec![
            json_response("200 OK", r#"{"status":"error","data":{"code":"invalid_cookies"}}"#),
            json_response("401 Unauthorized", "{}"),
        ]);
        let c = client(&base, Duration::from_secs(5));
        assert_eq!(c.search("q", 10, &ALL).await.unwrap_err(), ProtoError::SessionExpired);
        assert_eq!(c.search("q", 10, &ALL).await.unwrap_err(), ProtoError::SessionExpired);
    }

    #[tokio::test]
    async fn search_query_is_logged_at_trace_only() {
        use crate::test_log::Capture;

        let reply = format!(r#"{{"status":"ok","data":{{"messages":{SET},"users":{EMPTY_SET},"chats":{EMPTY_SET}}}}}"#);
        let (base, _) = serve(vec![json_response("200 OK", &reply), json_response("200 OK", &reply)]);
        let c = client(&base, Duration::from_secs(5));
        let debug = Capture::default();
        {
            let _guard = debug.install_at(tracing::Level::DEBUG);
            c.search("QUERY-WORD", 10, &ALL).await.unwrap();
        }
        assert!(debug.text().contains("registry exchange"), "{}", debug.text());
        debug.assert_absent(&["QUERY-WORD"]);
        let trace = Capture::default();
        {
            let _guard = trace.install();
            c.search("QUERY-WORD", 10, &ALL).await.unwrap();
        }
        assert!(trace.text().contains("registry reply body"), "{}", trace.text());
    }

    #[tokio::test]
    async fn log_shows_exchanges_but_no_secrets() {
        use crate::test_log::Capture;

        let capture = Capture::default();
        let _guard = capture.install();
        let (base, _) = serve(vec![
            json_response("200 OK", r#"{"token":"t0k-CSRF-VALUE"}"#),
            json_response("200 OK", r#"{"status":"ok","data":{"user":{"guid":"g-reply"}}}"#),
        ]);
        let mut endpoints = RegistryEndpoints::production();
        endpoints.api_url = format!("{base}/api/?uid=Q-VALUE-1");
        endpoints.csrf_url = format!("{base}/csrf/?uid=Q-VALUE-2");
        let cookie = SessionCookie { value: "c00kie-VALUE".into(), domain: "127.0.0.1".into(), ..cookie() };
        let c = RegistryClient::new(&endpoints, &HttpSettings::default(), &[cookie]).unwrap();
        c.execute(&ReadRequest::RequestUser).await.unwrap();

        let text = capture.text();
        for needle in ["csrf-token exchange", "registry exchange", "127.0.0.1/csrf/", "127.0.0.1/api/", "status=200", "request_user", "g-reply"] {
            assert!(text.contains(needle), "{needle}: {text}");
        }
        capture.assert_absent(&["c00kie-VALUE", "t0k-CSRF-VALUE", "Q-VALUE-1", "Q-VALUE-2"]);
    }
}
