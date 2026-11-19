//! Media fetch: avatars and image thumbnails. Downloading file attachments is not supported.

use reqwest::header::COOKIE;

use crate::error::ProtoError;
use crate::net::{HttpSettings, cookie_value, host_of, http_client};
use crate::registry::http_error;
use crate::session::{SessionCookie, cookie_header};

/// Responses larger than this are refused: thumbnails are far below it.
const MAX_BYTES: usize = 25 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct MediaConfig {
    pub files_base: String,
    pub avatars_base: String,
}

impl MediaConfig {
    pub fn production() -> Self {
        Self {
            files_base: "https://files.messenger.yandex.ru".into(),
            avatars_base: "https://avatars.mds.yandex.net".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarSize {
    Small,
    Large,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaTarget {
    /// `AvatarId` of a user or chat.
    Avatar { id: String, size: AvatarSize },
    /// `FileInfo.Id2` of an image message (`<bucket>/<uuid>`).
    Thumbnail { file_id: String },
}

/// One path segment of an id: refuses anything that could change the URL structure.
fn safe_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment.bytes().all(|b| b.is_ascii_alphanumeric() || b"._~@=-".contains(&b))
}

fn safe_path(id: &str) -> Result<&str, ProtoError> {
    if id.split('/').all(safe_segment) {
        Ok(id)
    } else {
        Err(ProtoError::Protocol("media id has unexpected characters".into()))
    }
}

impl MediaTarget {
    pub fn url(&self, config: &MediaConfig) -> Result<String, ProtoError> {
        match self {
            Self::Thumbnail { file_id } => {
                Ok(format!("{}/file_shortterm/{}?size=MIDDLE2048", config.files_base, safe_path(file_id)?))
            }
            Self::Avatar { id, size } => {
                let (mds, files) = match size {
                    AvatarSize::Small => ("islands-34", "SMALL"),
                    AvatarSize::Large => ("islands-50", "SMALL48"),
                };
                let safe = safe_path(id)?;
                Ok(match safe.strip_prefix("user_avatar/") {
                    Some(rest) if rest.starts_with("yapic/") || rest.starts_with("mssngr/") => {
                        let (namespace, image) = rest.split_once('/').unwrap_or_default();
                        format!("{}/get-{namespace}/{image}/{mds}", config.avatars_base)
                    }
                    _ => format!("{}/{safe}?size={files}", config.files_base),
                })
            }
        }
    }
}

#[derive(Clone)]
pub struct MediaClient {
    http: reqwest::Client,
    config: MediaConfig,
    cookies: Vec<SessionCookie>,
}

impl MediaClient {
    pub fn new(config: MediaConfig, settings: &HttpSettings, cookies: Vec<SessionCookie>) -> Result<Self, ProtoError> {
        Ok(Self { http: http_client(settings)?, config, cookies })
    }

    pub async fn fetch(&self, target: &MediaTarget) -> Result<Vec<u8>, ProtoError> {
        let url = target.url(&self.config)?;
        let mut request = self.http.get(&url);
        // Cookies go only where they match: public avatar hosts get none.
        if !cookie_header(&self.cookies, &host_of(&url)?).is_empty() {
            request = request.header(COOKIE, cookie_value(&self.cookies, &host_of(&url)?)?);
        }
        let mut response = request.send().await?;
        match response.status().as_u16() {
            200..=299 => {}
            401 => return Err(ProtoError::SessionExpired),
            status => return Err(http_error("media", status)),
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > MAX_BYTES {
                return Err(ProtoError::Protocol("media response is too large".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::registry::stub::serve;

    fn prod() -> MediaConfig {
        MediaConfig::production()
    }

    #[test]
    fn urls_follow_the_web_clients_templates() {
        let thumb = MediaTarget::Thumbnail { file_id: "bucket/0a1b-2c".into() };
        assert_eq!(thumb.url(&prod()).unwrap(), "https://files.messenger.yandex.ru/file_shortterm/bucket/0a1b-2c?size=MIDDLE2048");
        let avatar = |id: &str, size| MediaTarget::Avatar { id: id.into(), size }.url(&prod()).unwrap();
        assert_eq!(avatar("user_avatar/yapic/123/abc", AvatarSize::Small), "https://avatars.mds.yandex.net/get-yapic/123/abc/islands-34");
        assert_eq!(avatar("user_avatar/mssngr/9/x", AvatarSize::Large), "https://avatars.mds.yandex.net/get-mssngr/9/x/islands-50");
        assert_eq!(avatar("team/7/xyz", AvatarSize::Large), "https://files.messenger.yandex.ru/team/7/xyz?size=SMALL48");
    }

    #[test]
    fn hostile_ids_are_refused() {
        for id in ["", "a//b", "../etc", "a/./b", "a?x=1", "a#f", "a b", "a\\b", "a/%2e%2e/b", "/abs"] {
            assert!(MediaTarget::Thumbnail { file_id: id.into() }.url(&prod()).is_err(), "{id:?}");
        }
    }

    fn client(base: &str, timeout: Duration) -> MediaClient {
        let config = MediaConfig { files_base: base.into(), avatars_base: base.into() };
        MediaClient::new(config, &HttpSettings { connect_timeout: timeout, request_timeout: timeout }, vec![]).unwrap()
    }

    fn bytes_response(status: &str, body: &[u8]) -> Option<String> {
        Some(format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), String::from_utf8_lossy(body)))
    }

    #[tokio::test]
    async fn fetches_bytes_and_maps_statuses() {
        let (base, seen) = serve(vec![
            bytes_response("200 OK", b"pixels"),
            bytes_response("401 Unauthorized", b""),
            bytes_response("403 Forbidden", b""),
            bytes_response("502 Bad Gateway", b""),
        ]);
        let c = client(&base, Duration::from_secs(5));
        let thumb = MediaTarget::Thumbnail { file_id: "b/u".into() };
        assert_eq!(c.fetch(&thumb).await.unwrap(), b"pixels");
        assert!(seen.lock().unwrap()[0].starts_with("GET /file_shortterm/b/u?size=MIDDLE2048"));
        assert_eq!(c.fetch(&thumb).await.unwrap_err(), ProtoError::SessionExpired);
        assert!(matches!(c.fetch(&thumb).await.unwrap_err(), ProtoError::Protocol(_)));
        assert!(matches!(c.fetch(&thumb).await.unwrap_err(), ProtoError::Network(_)));
    }

    #[tokio::test]
    async fn silent_server_times_out() {
        let (base, _) = serve(vec![None]);
        let err = client(&base, Duration::from_millis(200)).fetch(&MediaTarget::Thumbnail { file_id: "b/u".into() }).await.unwrap_err();
        assert!(matches!(err, ProtoError::Network(_)));
    }
}
