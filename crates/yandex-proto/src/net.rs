use std::time::{Duration, Instant};

use reqwest::header::HeaderValue;

use crate::error::ProtoError;
use crate::session::{SessionCookie, cookie_header};

/// Finite deadlines for every HTTP client built by this crate: a silent server must
/// surface as a network error, never as a hung request.
#[derive(Clone, Copy, Debug)]
pub struct HttpSettings {
    pub connect_timeout: Duration,
    /// Covers the whole exchange, response body included. Streaming downloads have no
    /// total deadline: for them this is the longest silence allowed between chunks.
    pub request_timeout: Duration,
}

impl Default for HttpSettings {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
        }
    }
}

pub(crate) fn http_client(settings: &HttpSettings) -> Result<reqwest::Client, ProtoError> {
    reqwest::Client::builder()
        .connect_timeout(settings.connect_timeout)
        .timeout(settings.request_timeout)
        .build()
        .map_err(ProtoError::from)
}

/// Client for large bodies: a connect deadline and an idle (between-chunks) deadline, but
/// no total deadline, so a slow but steady transfer is not cut off while a stalled one fails.
pub(crate) fn http_client_streaming(
    settings: &HttpSettings,
) -> Result<reqwest::Client, ProtoError> {
    reqwest::Client::builder()
        .connect_timeout(settings.connect_timeout)
        .read_timeout(settings.request_timeout)
        .build()
        .map_err(ProtoError::from)
}

/// Sensitive `Cookie` header for `host`.
pub(crate) fn cookie_value(
    cookies: &[SessionCookie],
    host: &str,
) -> Result<HeaderValue, ProtoError> {
    let mut value = HeaderValue::from_str(&cookie_header(cookies, host))
        .map_err(|_| ProtoError::Protocol("cookie value is not a valid header".into()))?;
    value.set_sensitive(true);
    Ok(value)
}

pub(crate) fn host_of(url: &str) -> Result<String, ProtoError> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .ok_or_else(|| ProtoError::Protocol("invalid url".into()))
}

/// `host/path` of `url` for log lines: scheme, port, credentials, query and fragment are
/// dropped because queries carry user ids, session ids and tokens.
pub(crate) fn redact_url(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(u) => format!("{}{}", u.host_str().unwrap_or("?"), u.path()),
        Err(_) => "<invalid url>".into(),
    }
}

/// Whole milliseconds for log fields, saturating instead of truncating.
pub(crate) fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

/// Logs an exchange that failed without a usable response (DNS, TLS, timeout, broken body)
/// with the fields its success line would carry, then returns the error without its URL.
/// Use as `map_err` before `?`: the caller's error no longer says where it happened.
pub(crate) fn failed<'a>(
    call: &'static str,
    method: &'static str,
    url: &'a str,
    started: Instant,
) -> impl FnOnce(reqwest::Error) -> reqwest::Error + 'a {
    move |error| {
        let error = error.without_url();
        tracing::warn!(
            call,
            method,
            url = %redact_url(url),
            elapsed_ms = millis(started.elapsed()),
            timeout = error.is_timeout(),
            error = %describe(&error),
            "http exchange failed"
        );
        error
    }
}

/// The error with its causes: reqwest's own text is only "error sending request".
/// Pass an error stripped by `without_url`.
pub(crate) fn describe(error: &reqwest::Error) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacted_url_keeps_only_host_and_path() {
        assert_eq!(
            redact_url("https://u:p@yandex.ru:8443/messenger/api/?token=abc#frag"),
            "yandex.ru/messenger/api/"
        );
        assert_eq!(
            redact_url("wss://push.yandex.ru/v2/subscribe/websocket?session=a-b&user=4001"),
            "push.yandex.ru/v2/subscribe/websocket"
        );
        assert_eq!(redact_url("not a url?secret=1"), "<invalid url>");
    }
}
