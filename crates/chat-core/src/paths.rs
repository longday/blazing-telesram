//! Per-account locations under the runtime directory.

use crate::model::UserId;
use std::path::{Path, PathBuf};

/// Makes `raw` safe as a single path component: everything outside `[A-Za-z0-9_-]`
/// (including `.` and `/`) becomes `%XX`, so no input can escape its directory.
pub fn escape_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn cache_path(runtime_dir: &Path, account: &UserId) -> PathBuf {
    runtime_dir.join("cache").join(format!("{}.sqlite", escape_component(&account.0)))
}

pub fn media_dir(runtime_dir: &Path, account: &UserId) -> PathBuf {
    runtime_dir.join("media").join(escape_component(&account.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_separators_and_dots() {
        assert_eq!(escape_component("123_a-B"), "123_a-B");
        assert_eq!(escape_component("../x/y"), "%2E%2E%2Fx%2Fy");
        assert_eq!(escape_component("0/0/guid"), "0%2F0%2Fguid");
    }

    #[test]
    fn account_paths_are_separate() {
        let root = Path::new("/r");
        let a = UserId("1".into());
        let b = UserId("2".into());
        assert_eq!(cache_path(root, &a), Path::new("/r/cache/1.sqlite"));
        assert_ne!(cache_path(root, &a), cache_path(root, &b));
        assert_eq!(media_dir(root, &b), Path::new("/r/media/2"));
    }
}
