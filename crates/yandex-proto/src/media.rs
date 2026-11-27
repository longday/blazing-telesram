//! Media transfer: avatars and image thumbnails (`fetch`, buffered), original files
//! (`download`, streamed to a writer) and the research probe's view of one download
//! (`download_exchange`: status, landing host and the headers that decide streaming).

use std::io::{self, Write};
use std::time::Instant;

use reqwest::header::{ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_TYPE, COOKIE};
use tracing::debug;

use crate::error::ProtoError;
use crate::net::{
    HttpSettings, cookie_value, failed, host_of, http_client, http_client_streaming, millis,
    redact_url,
};
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
    /// `FileInfo.Id2` of a file or image message: the original, streamed by `download`.
    Original { file_id: String },
    /// `FileInfo.Id2` of a voice message: `file_shortterm/{id}` with no query, the web
    /// client's `voicePlayUrl`.
    Playback { file_id: String },
    /// `FileInfo.Id2` of a video message: `video_streams/{id}`, the web client's
    /// `videoStreamsUrl`.
    VideoStream { file_id: String },
}

/// One path segment of an id: refuses anything that could change the URL structure.
fn safe_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._~@=-".contains(&b))
}

fn safe_path(id: &str) -> Result<&str, ProtoError> {
    if id.split('/').all(safe_segment) {
        Ok(id)
    } else {
        Err(ProtoError::Protocol(
            "media id has unexpected characters".into(),
        ))
    }
}

impl MediaTarget {
    pub fn url(&self, config: &MediaConfig) -> Result<String, ProtoError> {
        match self {
            Self::Thumbnail { file_id } => Ok(format!(
                "{}/file_shortterm/{}?size=MIDDLE2048",
                config.files_base,
                safe_path(file_id)?
            )),
            Self::Original { file_id } => Ok(format!(
                "{}/file_shortterm/{}?attach=true",
                config.files_base,
                safe_path(file_id)?
            )),
            Self::Playback { file_id } => Ok(format!(
                "{}/file_shortterm/{}",
                config.files_base,
                safe_path(file_id)?
            )),
            Self::VideoStream { file_id } => Ok(format!(
                "{}/video_streams/{}",
                config.files_base,
                safe_path(file_id)?
            )),
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

/// What the server answered to one request, without the body, the query or any name: the
/// research probe prints exactly these fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exchange {
    /// The status after redirects were followed; any status is reported, none is an error.
    pub status: u16,
    /// Host (no port, no path) the redirects ended on.
    pub landed_host: String,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    pub accept_ranges: Option<String>,
    /// Only whether the header is present: its value carries the file name.
    pub has_content_disposition: bool,
    /// Bytes written to `out`; zero unless the status is 2xx.
    pub received: u64,
}

#[derive(Clone)]
pub struct MediaClient {
    http: reqwest::Client,
    /// Idle timeout instead of a total deadline: originals have no bounded size or duration.
    streaming: reqwest::Client,
    config: MediaConfig,
    cookies: Vec<SessionCookie>,
}

impl MediaClient {
    pub fn new(
        config: MediaConfig,
        settings: &HttpSettings,
        cookies: Vec<SessionCookie>,
    ) -> Result<Self, ProtoError> {
        Ok(Self {
            http: http_client(settings)?,
            streaming: http_client_streaming(settings)?,
            config,
            cookies,
        })
    }

    /// Sends the GET shared by every request and logs the exchange. No status is mapped.
    /// Returns the response with the start time.
    async fn send(
        &self,
        http: &reqwest::Client,
        url: &str,
    ) -> Result<(reqwest::Response, Instant), ProtoError> {
        let mut request = http.get(url);
        // Cookies go only where they match: public avatar hosts get none. reqwest drops the
        // header again when a redirect leads to another host or port.
        let host = host_of(url)?;
        if !cookie_header(&self.cookies, &host).is_empty() {
            request = request.header(COOKIE, cookie_value(&self.cookies, &host)?);
        }
        let started = Instant::now();
        let response = request
            .send()
            .await
            .map_err(failed("media", "GET", url, started))?;
        // `elapsed_ms` is the time to the response headers; the body streams after it.
        // `landed` is where redirects ended up; like the request, host and path only.
        debug!(
            method = "GET",
            url = %redact_url(url),
            landed = %redact_url(response.url().as_str()),
            status = response.status().as_u16(),
            elapsed_ms = millis(started.elapsed()),
            "media exchange"
        );
        Ok((response, started))
    }

    /// `send` plus the status line of `fetch` and `download`: 2xx passes, 401 is an expired
    /// session, 404 is `NotFound`.
    async fn get(
        &self,
        http: &reqwest::Client,
        url: &str,
    ) -> Result<(reqwest::Response, Instant), ProtoError> {
        let (response, started) = self.send(http, url).await?;
        match response.status().as_u16() {
            200..=299 => Ok((response, started)),
            401 => Err(ProtoError::SessionExpired),
            404 => Err(ProtoError::NotFound),
            status => Err(http_error("media", status)),
        }
    }

    pub async fn fetch(&self, target: &MediaTarget) -> Result<Vec<u8>, ProtoError> {
        let url = target.url(&self.config)?;
        let (mut response, started) = self.get(&self.http, &url).await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(failed("media", "GET", &url, started))?
        {
            if bytes.len() + chunk.len() > MAX_BYTES {
                return Err(ProtoError::Protocol("media response is too large".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    /// Streams the body into `out` chunk by chunk, never holding more than one chunk in
    /// memory and without a size cap. `progress(received, content_length)` runs after every
    /// chunk. Returns the byte count. A body shorter than the announced `Content-Length` is
    /// `Network`; a failing `out` is `Storage`. Dropping the future stops the transfer.
    pub async fn download(
        &self,
        target: &MediaTarget,
        out: &mut (dyn Write + Send),
        progress: &(dyn Fn(u64, Option<u64>) + Send + Sync),
    ) -> Result<u64, ProtoError> {
        let url = target.url(&self.config)?;
        let (response, started) = self.get(&self.streaming, &url).await?;
        copy_body(response, started, &url, out, progress).await
    }

    /// One streamed request for the research probe: the body goes into `out` only for a 2xx
    /// answer, every status (a refusal included) comes back as data, and only a failed
    /// transfer is an error. The same truncation and `Storage` rules as [`Self::download`].
    pub async fn download_exchange(
        &self,
        target: &MediaTarget,
        out: &mut (dyn Write + Send),
    ) -> Result<Exchange, ProtoError> {
        let url = target.url(&self.config)?;
        let (response, started) = self.send(&self.streaming, &url).await?;
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let mut exchange = Exchange {
            status: response.status().as_u16(),
            landed_host: response.url().host_str().unwrap_or("?").to_owned(),
            content_type: header(CONTENT_TYPE),
            content_length: response.content_length(),
            accept_ranges: header(ACCEPT_RANGES),
            has_content_disposition: response.headers().contains_key(CONTENT_DISPOSITION),
            received: 0,
        };
        if response.status().is_success() {
            exchange.received = copy_body(response, started, &url, out, &|_, _| {}).await?;
        }
        Ok(exchange)
    }
}

/// Copies the body of a 2xx response into `out` and returns the byte count.
async fn copy_body(
    mut response: reqwest::Response,
    started: Instant,
    url: &str,
    out: &mut (dyn Write + Send),
    progress: &(dyn Fn(u64, Option<u64>) + Send + Sync),
) -> Result<u64, ProtoError> {
    let total = response.content_length();
    let mut received = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(failed("media", "GET", url, started))?
    {
        out.write_all(&chunk).map_err(storage_error)?;
        received += chunk.len() as u64;
        progress(received, total);
    }
    out.flush().map_err(storage_error)?;
    if total.is_some_and(|total| total != received) {
        return Err(ProtoError::Network("response ended early".into()));
    }
    Ok(received)
}

/// Only the error kind is kept: an `io::Error` message can carry the destination path.
fn storage_error(error: io::Error) -> ProtoError {
    ProtoError::Storage(error.kind().to_string())
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use super::*;
    use crate::registry::stub::serve;

    fn prod() -> MediaConfig {
        MediaConfig::production()
    }

    #[test]
    fn urls_follow_the_web_clients_templates() {
        let thumb = MediaTarget::Thumbnail {
            file_id: "bucket/0a1b-2c".into(),
        };
        assert_eq!(
            thumb.url(&prod()).unwrap(),
            "https://files.messenger.yandex.ru/file_shortterm/bucket/0a1b-2c?size=MIDDLE2048"
        );
        let avatar = |id: &str, size| {
            MediaTarget::Avatar {
                id: id.into(),
                size,
            }
            .url(&prod())
            .unwrap()
        };
        assert_eq!(
            avatar("user_avatar/yapic/123/abc", AvatarSize::Small),
            "https://avatars.mds.yandex.net/get-yapic/123/abc/islands-34"
        );
        assert_eq!(
            avatar("user_avatar/mssngr/9/x", AvatarSize::Large),
            "https://avatars.mds.yandex.net/get-mssngr/9/x/islands-50"
        );
        assert_eq!(
            avatar("team/7/xyz", AvatarSize::Large),
            "https://files.messenger.yandex.ru/team/7/xyz?size=SMALL48"
        );
    }

    #[test]
    fn hostile_ids_are_refused() {
        for id in [
            "",
            "a//b",
            "../etc",
            "a/./b",
            "a?x=1",
            "a#f",
            "a b",
            "a\\b",
            "a/%2e%2e/b",
            "/abs",
        ] {
            assert!(
                MediaTarget::Thumbnail { file_id: id.into() }
                    .url(&prod())
                    .is_err(),
                "{id:?}"
            );
        }
    }

    fn client(base: &str, timeout: Duration) -> MediaClient {
        let config = MediaConfig {
            files_base: base.into(),
            avatars_base: base.into(),
        };
        MediaClient::new(
            config,
            &HttpSettings {
                connect_timeout: timeout,
                request_timeout: timeout,
            },
            vec![],
        )
        .unwrap()
    }

    fn bytes_response(status: &str, body: &[u8]) -> Option<String> {
        Some(format!(
            "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        ))
    }

    #[tokio::test]
    async fn fetches_bytes_and_maps_statuses() {
        let (base, seen) = serve(vec![
            bytes_response("200 OK", b"pixels"),
            bytes_response("401 Unauthorized", b""),
            bytes_response("403 Forbidden", b""),
            bytes_response("404 Not Found", b""),
            bytes_response("502 Bad Gateway", b""),
        ]);
        let c = client(&base, Duration::from_secs(5));
        let thumb = MediaTarget::Thumbnail {
            file_id: "b/u".into(),
        };
        assert_eq!(c.fetch(&thumb).await.unwrap(), b"pixels");
        assert!(seen.lock().unwrap()[0].starts_with("GET /file_shortterm/b/u?size=MIDDLE2048"));
        assert_eq!(
            c.fetch(&thumb).await.unwrap_err(),
            ProtoError::SessionExpired
        );
        assert!(matches!(
            c.fetch(&thumb).await.unwrap_err(),
            ProtoError::Protocol(_)
        ));
        assert_eq!(c.fetch(&thumb).await.unwrap_err(), ProtoError::NotFound);
        assert!(matches!(
            c.fetch(&thumb).await.unwrap_err(),
            ProtoError::Network(_)
        ));
    }

    #[tokio::test]
    async fn silent_server_times_out() {
        let (base, _) = serve(vec![None]);
        let err = client(&base, Duration::from_millis(200))
            .fetch(&MediaTarget::Thumbnail {
                file_id: "b/u".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ProtoError::Network(_)));
    }

    #[tokio::test]
    async fn log_shows_the_request_without_its_query() {
        use crate::test_log::Capture;

        let capture = Capture::default();
        let _guard = capture.install_at(tracing::Level::DEBUG);
        let (base, _) = serve(vec![bytes_response("200 OK", b"pixels-BODY")]);
        client(&base, Duration::from_secs(5))
            .fetch(&MediaTarget::Thumbnail {
                file_id: "b/u".into(),
            })
            .await
            .unwrap();
        let text = capture.text();
        for needle in [
            "media exchange",
            "127.0.0.1/file_shortterm/b/u",
            "status=200",
            "elapsed_ms",
        ] {
            assert!(text.contains(needle), "{needle}: {text}");
        }
        capture.assert_absent(&["MIDDLE2048", "size=", "pixels-BODY"]);
    }

    // ---- original downloads ----

    fn original() -> MediaTarget {
        MediaTarget::Original {
            file_id: "b/u".into(),
        }
    }

    #[test]
    fn original_url_is_the_attachment_template() {
        let url = MediaTarget::Original {
            file_id: "bucket/0a1b-2c".into(),
        }
        .url(&prod())
        .unwrap();
        assert_eq!(
            url,
            "https://files.messenger.yandex.ru/file_shortterm/bucket/0a1b-2c?attach=true"
        );
    }

    #[test]
    fn hostile_original_ids_are_refused() {
        for id in [
            "",
            "a//b",
            "../etc",
            "a/./b",
            "a?x=1",
            "a#f",
            "a b",
            "a\\b",
            "a/%2e%2e/b",
            "/abs",
        ] {
            assert!(
                MediaTarget::Original { file_id: id.into() }
                    .url(&prod())
                    .is_err(),
                "{id:?}"
            );
        }
    }

    fn cookie(domain: &str) -> SessionCookie {
        SessionCookie {
            name: "Session_id".into(),
            value: "s3cret-cookie".into(),
            domain: domain.into(),
            path: "/".into(),
            expires_unix: None,
            secure: true,
            http_only: true,
        }
    }

    fn client_with(base: &str, timeout: Duration, cookies: Vec<SessionCookie>) -> MediaClient {
        let config = MediaConfig {
            files_base: base.into(),
            avatars_base: base.into(),
        };
        MediaClient::new(
            config,
            &HttpSettings {
                connect_timeout: timeout,
                request_timeout: timeout,
            },
            cookies,
        )
        .unwrap()
    }

    /// Serves one connection: `head`, then each piece after its delay, then holds the
    /// connection open for `hold` before closing. Records the raw request.
    fn serve_script(
        head: &str,
        pieces: Vec<(Duration, Vec<u8>)>,
        hold: Duration,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let head = head.to_owned();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut buf = vec![0u8; 16384];
            let n = stream.read(&mut buf).unwrap_or(0);
            log.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[..n]).into_owned());
            let _ = stream.write_all(head.as_bytes());
            for (delay, piece) in pieces {
                thread::sleep(delay);
                if stream
                    .write_all(&piece)
                    .and_then(|()| stream.flush())
                    .is_err()
                {
                    return;
                }
            }
            thread::sleep(hold);
        });
        (base, seen)
    }

    fn ok_head(length: usize) -> String {
        format!("HTTP/1.1 200 OK\r\ncontent-length: {length}\r\nconnection: close\r\n\r\n")
    }

    /// A sink that only counts, so a big body is not held in memory by the test.
    struct Count(u64);

    impl Write for Count {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0 += buf.len() as u64;
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct Broken(io::ErrorKind);

    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(
                self.0,
                "/Users/someone/Downloads/secret-name.pdf.part",
            ))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn downloads_bytes_and_reports_progress_with_the_total() {
        let pieces = ["ab", "cd", "ef"]
            .map(|p| (Duration::from_millis(60), p.as_bytes().to_vec()))
            .to_vec();
        let (base, seen) = serve_script(&ok_head(6), pieces, Duration::ZERO);
        let steps = Mutex::new(Vec::new());
        let mut out = Vec::new();
        let n = client_with(&base, Duration::from_secs(5), vec![])
            .download(&original(), &mut out, &|received, total| {
                steps.lock().unwrap().push((received, total))
            })
            .await
            .unwrap();
        assert_eq!((n, out.as_slice()), (6, b"abcdef".as_slice()));
        assert!(seen.lock().unwrap()[0].starts_with("GET /file_shortterm/b/u?attach=true"));
        let steps = steps.into_inner().unwrap();
        assert!(steps.len() >= 2, "{steps:?}");
        assert!(steps.windows(2).all(|w| w[0].0 < w[1].0), "{steps:?}");
        assert!(
            steps.iter().all(|&(_, total)| total == Some(6)),
            "{steps:?}"
        );
        assert_eq!(steps.last(), Some(&(6, Some(6))));
    }

    #[tokio::test]
    async fn progress_has_no_total_when_the_length_is_not_announced() {
        let head = "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n";
        let pieces = vec![
            (Duration::from_millis(30), b"5\r\nhello\r\n".to_vec()),
            (Duration::from_millis(30), b"0\r\n\r\n".to_vec()),
        ];
        let (base, _) = serve_script(head, pieces, Duration::ZERO);
        let steps = Mutex::new(Vec::new());
        let mut out = Vec::new();
        let n = client_with(&base, Duration::from_secs(5), vec![])
            .download(&original(), &mut out, &|received, total| {
                steps.lock().unwrap().push((received, total))
            })
            .await
            .unwrap();
        assert_eq!((n, out.as_slice()), (5, b"hello".as_slice()));
        assert_eq!(steps.into_inner().unwrap(), vec![(5, None)]);
    }

    #[tokio::test]
    async fn a_body_above_the_preview_cap_is_accepted() {
        let len = MAX_BYTES + 1024 * 1024;
        let (base, _) = serve_script(
            &ok_head(len),
            vec![(Duration::ZERO, vec![7u8; len])],
            Duration::ZERO,
        );
        let mut out = Count(0);
        let last = Mutex::new(None);
        let n = client_with(&base, Duration::from_secs(20), vec![])
            .download(&original(), &mut out, &|received, total| {
                *last.lock().unwrap() = Some((received, total))
            })
            .await
            .unwrap();
        assert_eq!((n, out.0), (len as u64, len as u64));
        assert_eq!(
            last.into_inner().unwrap(),
            Some((len as u64, Some(len as u64)))
        );
    }

    #[tokio::test]
    async fn download_maps_statuses_like_fetch() {
        let (base, _) = serve(vec![
            bytes_response("401 Unauthorized", b""),
            bytes_response("403 Forbidden", b""),
            bytes_response("404 Not Found", b""),
            bytes_response("502 Bad Gateway", b""),
        ]);
        let c = client_with(&base, Duration::from_secs(5), vec![]);
        let mut out = Vec::new();
        let mut attempt = async || {
            c.download(&original(), &mut out, &|_, _| {})
                .await
                .unwrap_err()
        };
        assert_eq!(attempt().await, ProtoError::SessionExpired);
        assert!(matches!(attempt().await, ProtoError::Protocol(_)));
        assert_eq!(attempt().await, ProtoError::NotFound);
        assert!(matches!(attempt().await, ProtoError::Network(_)));
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn a_truncated_body_fails() {
        let (base, _) = serve_script(
            &ok_head(10),
            vec![(Duration::ZERO, b"12345".to_vec())],
            Duration::ZERO,
        );
        let mut out = Vec::new();
        let err = client_with(&base, Duration::from_secs(5), vec![])
            .download(&original(), &mut out, &|_, _| {})
            .await
            .unwrap_err();
        assert!(matches!(err, ProtoError::Network(_)), "{err:?}");
    }

    #[tokio::test]
    async fn a_stalled_body_fails_after_the_idle_timeout() {
        let (base, _) = serve_script(
            &ok_head(10),
            vec![(Duration::ZERO, b"12345".to_vec())],
            Duration::from_secs(5),
        );
        let started = Instant::now();
        let mut out = Vec::new();
        let err = client_with(&base, Duration::from_millis(300), vec![])
            .download(&original(), &mut out, &|_, _| {})
            .await
            .unwrap_err();
        assert!(matches!(err, ProtoError::Network(_)), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn a_slow_but_steady_body_outlives_the_idle_timeout() {
        // Eight pieces 100 ms apart take longer than the 350 ms idle timeout in total.
        let pieces = (0..8)
            .map(|_| (Duration::from_millis(100), b"xy".to_vec()))
            .collect();
        let (base, _) = serve_script(&ok_head(16), pieces, Duration::ZERO);
        let started = Instant::now();
        let mut out = Vec::new();
        let n = client_with(&base, Duration::from_millis(350), vec![])
            .download(&original(), &mut out, &|_, _| {})
            .await
            .unwrap();
        assert_eq!(n, 16);
        assert!(started.elapsed() > Duration::from_millis(350));
    }

    #[tokio::test]
    async fn a_redirect_to_another_host_drops_the_cookie() {
        use crate::test_log::Capture;

        let (storage, storage_seen) = serve(vec![bytes_response("200 OK", b"payload")]);
        let redirect = format!(
            "HTTP/1.1 302 Found\r\nlocation: {storage}/store/blob?sig=abc\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        );
        let (files, files_seen) = serve(vec![Some(redirect)]);
        assert_ne!(files, storage, "two servers on different ports");
        let capture = Capture::default();
        let _guard = capture.install_at(tracing::Level::DEBUG);
        let mut out = Vec::new();
        let n = client_with(&files, Duration::from_secs(5), vec![cookie("127.0.0.1")])
            .download(&original(), &mut out, &|_, _| {})
            .await
            .unwrap();
        assert_eq!((n, out.as_slice()), (7, b"payload".as_slice()));
        let first = files_seen.lock().unwrap()[0].to_lowercase();
        assert!(
            first.contains("cookie: session_id=s3cret-cookie"),
            "{first}"
        );
        let second = storage_seen.lock().unwrap()[0].to_lowercase();
        assert!(second.starts_with("get /store/blob?sig=abc"), "{second}");
        assert!(!second.contains("cookie"), "{second}");
        assert!(
            capture.text().contains("landed=127.0.0.1/store/blob"),
            "{}",
            capture.text()
        );
        capture.assert_absent(&["sig=abc", "s3cret-cookie"]);
    }

    #[tokio::test]
    async fn log_shows_the_download_without_its_query_cookie_or_body() {
        use crate::test_log::Capture;

        let capture = Capture::default();
        let _guard = capture.install_at(tracing::Level::DEBUG);
        let cookies = vec![cookie("127.0.0.1")];
        let (base, _) = serve(vec![bytes_response("200 OK", b"pixels-BODY")]);
        let mut out = Vec::new();
        client_with(&base, Duration::from_secs(5), cookies.clone())
            .download(&original(), &mut out, &|_, _| {})
            .await
            .unwrap();
        let text = capture.text();
        for needle in [
            "media exchange",
            "127.0.0.1/file_shortterm/b/u",
            "status=200",
            "elapsed_ms",
        ] {
            assert!(text.contains(needle), "{needle}: {text}");
        }
        // A failed transfer logs its own record, equally free of query, cookie and body.
        let (base, _) = serve_script(
            &ok_head(20),
            vec![(Duration::ZERO, b"pixels-BODY".to_vec())],
            Duration::ZERO,
        );
        client_with(&base, Duration::from_secs(5), cookies)
            .download(&original(), &mut out, &|_, _| {})
            .await
            .unwrap_err();
        assert!(
            capture.text().contains("http exchange failed"),
            "{}",
            capture.text()
        );
        capture.assert_absent(&["attach", "s3cret-cookie", "pixels-BODY"]);
    }

    #[tokio::test]
    async fn a_failing_writer_is_a_storage_error_without_the_destination() {
        let (base, _) = serve(vec![bytes_response("200 OK", b"pixels")]);
        let mut out = Broken(io::ErrorKind::StorageFull);
        let err = client_with(&base, Duration::from_secs(5), vec![])
            .download(&original(), &mut out, &|_, _| {})
            .await
            .unwrap_err();
        assert_eq!(
            err,
            ProtoError::Storage(io::ErrorKind::StorageFull.to_string())
        );
        assert!(!err.to_string().contains("secret-name"), "{err}");
    }

    // ---- playback and video stream targets, the research exchange ----

    fn playback_and_stream(id: &str) -> [MediaTarget; 2] {
        [
            MediaTarget::Playback { file_id: id.into() },
            MediaTarget::VideoStream { file_id: id.into() },
        ]
    }

    #[test]
    fn playback_is_the_plain_url_and_the_video_stream_its_own_path() {
        let [playback, stream] = playback_and_stream("bucket/0a1b-2c");
        assert_eq!(
            playback.url(&prod()).unwrap(),
            "https://files.messenger.yandex.ru/file_shortterm/bucket/0a1b-2c"
        );
        assert_eq!(
            stream.url(&prod()).unwrap(),
            "https://files.messenger.yandex.ru/video_streams/bucket/0a1b-2c"
        );
        for target in [playback, stream] {
            assert!(!target.url(&prod()).unwrap().contains('?'), "{target:?}");
        }
    }

    #[test]
    fn every_id_segment_is_checked_for_playback_and_the_video_stream() {
        for id in [
            "",
            "a//b",
            "../etc",
            "a/../b",
            "a/./b",
            "a/b/",
            "a?attach=true",
            "a#f",
            "a b",
            "a\\b",
            "a%2fb",
            "a/%2e%2e/b",
            "/abs",
            "a\0b",
            "a\nb",
            "é/b",
        ] {
            for target in playback_and_stream(id) {
                assert!(target.url(&prod()).is_err(), "{target:?}");
            }
        }
    }

    fn full_head(extra: &str, length: usize) -> String {
        format!("HTTP/1.1 200 OK\r\n{extra}content-length: {length}\r\nconnection: close\r\n\r\n")
    }

    #[tokio::test]
    async fn the_exchange_reports_status_headers_host_and_byte_count() {
        let head = full_head(
            "content-type: audio/ogg\r\naccept-ranges: bytes\r\ncontent-disposition: attachment; filename=\"secret-name.ogg\"\r\n",
            6,
        );
        let (base, seen) = serve_script(
            &head,
            vec![(Duration::ZERO, b"abcdef".to_vec())],
            Duration::ZERO,
        );
        let target = MediaTarget::Playback {
            file_id: "b/u".into(),
        };
        let mut out = Vec::new();
        let exchange = client_with(&base, Duration::from_secs(5), vec![])
            .download_exchange(&target, &mut out)
            .await
            .unwrap();
        assert_eq!(
            exchange,
            Exchange {
                status: 200,
                landed_host: "127.0.0.1".into(),
                content_type: Some("audio/ogg".into()),
                content_length: Some(6),
                accept_ranges: Some("bytes".into()),
                has_content_disposition: true,
                received: 6,
            }
        );
        assert!(!format!("{exchange:?}").contains("secret-name"));
        assert_eq!(out, b"abcdef");
        assert!(seen.lock().unwrap()[0].starts_with("GET /file_shortterm/b/u HTTP/1.1"));
    }

    #[tokio::test]
    async fn absent_headers_are_none_and_a_refusal_is_data_without_a_body() {
        let (base, _) = serve(vec![
            bytes_response("403 Forbidden", b"denied-BODY"),
            Some("HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n3\r\nabc\r\n0\r\n\r\n".into()),
        ]);
        let c = client_with(&base, Duration::from_secs(5), vec![]);
        let stream = MediaTarget::VideoStream {
            file_id: "b/u".into(),
        };
        let mut out = Vec::new();
        let refused = c.download_exchange(&stream, &mut out).await.unwrap();
        assert_eq!(
            (
                refused.status,
                refused.received,
                refused.has_content_disposition
            ),
            (403, 0, false)
        );
        assert!(out.is_empty());
        let open = c.download_exchange(&stream, &mut out).await.unwrap();
        assert_eq!(
            (
                open.status,
                open.content_type,
                open.content_length,
                open.accept_ranges,
                open.received
            ),
            (200, None, None, None, 3)
        );
        assert_eq!(out, b"abc");
    }

    #[tokio::test]
    async fn the_exchange_names_the_host_a_redirect_landed_on() {
        let (target_base, _) = serve(vec![bytes_response("200 OK", b"moved")]);
        let port = target_base.rsplit(':').next().unwrap();
        let redirect = format!(
            "HTTP/1.1 302 Found\r\nlocation: http://localhost:{port}/storage/x?sig=secret\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        );
        let (base, _) = serve(vec![Some(redirect)]);
        let mut out = Vec::new();
        let exchange = client_with(&base, Duration::from_secs(5), vec![])
            .download_exchange(
                &MediaTarget::Playback {
                    file_id: "b/u".into(),
                },
                &mut out,
            )
            .await
            .unwrap();
        assert_eq!(
            (
                exchange.status,
                exchange.landed_host.as_str(),
                out.as_slice()
            ),
            (200, "localhost", b"moved".as_slice())
        );
    }

    #[tokio::test]
    async fn a_truncated_exchange_fails_like_a_download() {
        let (base, _) = serve_script(
            &ok_head(10),
            vec![(Duration::ZERO, b"12345".to_vec())],
            Duration::ZERO,
        );
        let mut out = Vec::new();
        let err = client_with(&base, Duration::from_secs(5), vec![])
            .download_exchange(&original(), &mut out)
            .await
            .unwrap_err();
        assert!(matches!(err, ProtoError::Network(_)), "{err:?}");
    }
}
