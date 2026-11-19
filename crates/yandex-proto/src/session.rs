use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::runtime::RuntimeDir;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session file I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("session file is corrupt: {0}")]
    Format(#[from] serde_json::Error),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SessionCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires_unix: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
}

impl fmt::Debug for SessionCookie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionCookie")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .field("domain", &self.domain)
            .field("path", &self.path)
            .field("expires_unix", &self.expires_unix)
            .field("secure", &self.secure)
            .field("http_only", &self.http_only)
            .finish()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Identity {
    pub uid: Option<String>,
    pub guid: String,
    pub display_name: Option<String>,
}

impl Identity {
    /// Value for the `user=` parameter of the push subscription: uid, else guid.
    pub fn xiva_user(&self) -> &str {
        self.uid.as_deref().unwrap_or(&self.guid)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub cookies: Vec<SessionCookie>,
    pub identity: Identity,
}

impl Session {
    /// `None` when no session has been stored.
    pub fn load(dir: &RuntimeDir) -> Result<Option<Session>, SessionError> {
        match fs::read(dir.session_file()) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes a temp file in the target directory and renames it over the session
    /// file, so a crash never leaves a partial session behind.
    pub fn save(&self, dir: &RuntimeDir) -> Result<(), SessionError> {
        let target = dir.session_file();
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir.path())?;
        let tmp = target.with_extension(format!("json.tmp{}", std::process::id()));
        let result = write_private(&tmp, &serde_json::to_vec(self)?)
            .and_then(|()| fs::rename(&tmp, &target));
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        Ok(result?)
    }

    pub fn delete(dir: &RuntimeDir) -> Result<(), SessionError> {
        match fs::remove_file(dir.session_file()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// `Cookie` header value for `host`: cookies whose domain is `host` or a parent
/// of it (a leading dot is ignored), skipping expired ones.
pub fn cookie_header(cookies: &[SessionCookie], host: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let host = host.to_ascii_lowercase();
    cookies
        .iter()
        .filter(|c| c.expires_unix.is_none_or(|t| t > now))
        .filter(|c| domain_matches(&c.domain, &host))
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ")
}

fn domain_matches(cookie_domain: &str, host: &str) -> bool {
    let domain = cookie_domain.trim_start_matches('.');
    host.strip_suffix(domain.to_ascii_lowercase().as_str())
        .is_some_and(|prefix| prefix.is_empty() || prefix.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn cookie(name: &str, value: &str, domain: &str) -> SessionCookie {
        SessionCookie {
            name: name.into(),
            value: value.into(),
            domain: domain.into(),
            path: "/".into(),
            expires_unix: Some(4_000_000_000),
            secure: true,
            http_only: true,
        }
    }

    fn sample() -> Session {
        Session {
            cookies: vec![
                cookie("Session_id", "SECRET_SESSION_VALUE", ".yandex.ru"),
                cookie("yandexuid", "SECRET_UID_VALUE", "yandex.ru"),
            ],
            identity: Identity {
                uid: Some("123".into()),
                guid: "guid-1".into(),
                display_name: Some("Ivan".into()),
            },
        }
    }

    #[test]
    fn round_trip_and_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = RuntimeDir::new(tmp.path().join("nested/.runtime"));
        assert!(Session::load(&dir).unwrap().is_none());

        let session = sample();
        session.save(&dir).unwrap();
        let loaded = Session::load(&dir).unwrap().unwrap();
        assert_eq!(loaded.identity, session.identity);
        assert_eq!(loaded.cookies.len(), 2);
        assert_eq!(loaded.cookies[0].value, "SECRET_SESSION_VALUE");
        assert_eq!(loaded.cookies[0].expires_unix, Some(4_000_000_000));
        assert!(loaded.cookies[0].http_only);

        // Overwrite leaves no temp files behind.
        session.save(&dir).unwrap();
        let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1);

        Session::delete(&dir).unwrap();
        assert!(Session::load(&dir).unwrap().is_none());
        Session::delete(&dir).unwrap();
    }

    #[test]
    fn files_are_private() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = RuntimeDir::new(tmp.path().join("rt"));
        sample().save(&dir).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir.session_file()), 0o600);
        assert_eq!(mode(dir.path()), 0o700);
    }

    #[test]
    fn corrupt_file_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = RuntimeDir::new(tmp.path());
        fs::write(dir.session_file(), b"{not json").unwrap();
        assert!(matches!(Session::load(&dir), Err(SessionError::Format(_))));
    }

    #[test]
    fn debug_hides_cookie_values() {
        let session = sample();
        for out in [format!("{session:?}"), format!("{:#?}", session.cookies[0])] {
            assert!(!out.contains("SECRET_SESSION_VALUE"), "{out}");
            assert!(!out.contains("SECRET_UID_VALUE"), "{out}");
            assert!(out.contains("Session_id"));
        }
    }

    #[test]
    fn cookie_header_matches_domains() {
        let cookies = vec![
            cookie("a", "1", ".yandex.ru"),
            cookie("b", "2", "push.yandex.ru"),
            cookie("c", "3", ".notyandex.ru"),
        ];
        assert_eq!(cookie_header(&cookies, "yandex.ru"), "a=1");
        assert_eq!(cookie_header(&cookies, "push.yandex.ru"), "a=1; b=2");
        assert_eq!(cookie_header(&cookies, "notyandex.ru"), "c=3");
        assert_eq!(cookie_header(&cookies, "other.example"), "");
    }

    #[test]
    fn cookie_header_skips_expired() {
        let mut expired = cookie("old", "x", ".yandex.ru");
        expired.expires_unix = Some(1);
        let cookies = vec![expired, cookie("new", "y", ".yandex.ru")];
        assert_eq!(cookie_header(&cookies, "yandex.ru"), "new=y");
    }

    #[test]
    fn xiva_user_prefers_uid() {
        let mut id = sample().identity;
        assert_eq!(id.xiva_user(), "123");
        id.uid = None;
        assert_eq!(id.xiva_user(), "guid-1");
    }
}
