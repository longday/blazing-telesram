use std::time::Duration;

use reqwest::header::HeaderValue;

use crate::error::ProtoError;
use crate::session::{SessionCookie, cookie_header};

/// Finite deadlines for every HTTP client built by this crate: a silent server must
/// surface as a network error, never as a hung request.
#[derive(Clone, Copy, Debug)]
pub struct HttpSettings {
    pub connect_timeout: Duration,
    /// Covers the whole exchange, response body included.
    pub request_timeout: Duration,
}

impl Default for HttpSettings {
    fn default() -> Self {
        Self { connect_timeout: Duration::from_secs(10), request_timeout: Duration::from_secs(30) }
    }
}

pub(crate) fn http_client(settings: &HttpSettings) -> Result<reqwest::Client, ProtoError> {
    reqwest::Client::builder()
        .connect_timeout(settings.connect_timeout)
        .timeout(settings.request_timeout)
        .build()
        .map_err(ProtoError::from)
}

/// Sensitive `Cookie` header for `host`.
pub(crate) fn cookie_value(cookies: &[SessionCookie], host: &str) -> Result<HeaderValue, ProtoError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacted_url_keeps_only_host_and_path() {
        assert_eq!(redact_url("https://u:p@yandex.ru:8443/messenger/api/?token=abc#frag"), "yandex.ru/messenger/api/");
        assert_eq!(redact_url("wss://push.yandex.ru/v2/subscribe/websocket?session=a-b&user=4001"), "push.yandex.ru/v2/subscribe/websocket");
        assert_eq!(redact_url("not a url?secret=1"), "<invalid url>");
    }
}
