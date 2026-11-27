//! Per-account locations under the runtime directory, and safe names for saved files.

use crate::model::UserId;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
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
    runtime_dir
        .join("cache")
        .join(format!("{}.sqlite", escape_component(&account.0)))
}

pub fn media_dir(runtime_dir: &Path, account: &UserId) -> PathBuf {
    runtime_dir.join("media").join(escape_component(&account.0))
}

/// Name given to a download whose own name is unusable (absent, empty, only dots).
const FALLBACK_FILE_NAME: &str = "Файл";
/// Longest file name this module produces, in bytes (APFS allows 255; the room left
/// is for the ` (N)` collision suffix and the `.part` suffix).
const MAX_NAME_BYTES: usize = 200;
/// Longest extension kept when a name is shortened or numbered, dot included.
const MAX_EXT_BYTES: usize = 16;
/// Collision suffixes tried before giving up, so a broken filesystem cannot loop forever.
const MAX_ATTEMPTS: u32 = 10_000;

/// Splits `name` into stem and extension (dot included). The extension is the text from
/// the last dot when that dot is not the first byte, the text is at most
/// [`MAX_EXT_BYTES`] and holds no whitespace; otherwise it is empty (`v1.2 final` has none).
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 => {
            let ext = &name[dot..];
            if ext.len() <= MAX_EXT_BYTES && !ext.contains(char::is_whitespace) {
                name.split_at(dot)
            } else {
                (name, "")
            }
        }
        _ => (name, ""),
    }
}

/// Largest prefix of `s` that is at most `max` bytes and ends on a `char` boundary.
fn cut_on_boundary(s: &str, max: usize) -> &str {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Turns the name a server announced into one safe path component: the last component
/// after `/` and `\`, control characters and `:` replaced with `_`, whitespace and leading
/// dots trimmed, at most 200 bytes (cut on a `char` boundary, the extension kept).
/// Empty, `.`, `..`, only dots and `None` give `"Файл"`. Idempotent.
pub fn sanitize_file_name(name: Option<&str>) -> String {
    let last = name.unwrap_or("").rsplit(['/', '\\']).next().unwrap_or("");
    let replaced: String = last
        .chars()
        .map(|c| if c.is_control() || c == ':' { '_' } else { c })
        .collect();
    let trimmed = replaced
        .trim_start_matches(|c: char| c == '.' || c.is_whitespace())
        .trim_end();
    if trimmed.is_empty() {
        return FALLBACK_FILE_NAME.to_owned();
    }
    if trimmed.len() <= MAX_NAME_BYTES {
        return trimmed.to_owned();
    }
    let (stem, ext) = split_extension(trimmed);
    let stem = cut_on_boundary(stem, MAX_NAME_BYTES - ext.len()).trim_end();
    format!("{stem}{ext}")
}

/// The `n`-th candidate for a taken name: `n == 0` is `name` itself, `n == 1` is
/// `report (1).pdf` for `report.pdf`, and so on. `name` is expected to be sanitized.
pub fn unique_name(name: &str, n: u32) -> String {
    if n == 0 {
        return name.to_owned();
    }
    let (stem, ext) = split_extension(name);
    format!("{stem} ({n}){ext}")
}

/// A file being downloaded, `<name>.part` inside the destination folder. Dropping it
/// removes the file unless [`commit_partial`] has moved it to its final name, so a
/// cancel, an error, an aborted task and a panic all clean up the same way.
#[derive(Debug)]
pub struct Partial {
    path: PathBuf,
    committed: bool,
}

impl Partial {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        if !self.committed {
            // Best effort: the file may already be gone, and Drop cannot report.
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn no_free_name() -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, "no free file name")
}

/// Creates the partial file for `name` in `dir` with `create_new`, so it never opens an
/// existing file: when `<name>.part` is taken (another download of the same name) the
/// partial becomes `<stem> (1).<ext>.part`, and so on. `name` is sanitized again here, so
/// no input can leave `dir`.
pub fn create_partial(dir: &Path, name: &str) -> io::Result<(File, Partial)> {
    create_partial_with_mode(dir, name, 0o666)
}

/// [`create_partial`] whose file is created with `mode` (before the umask), so a private file
/// is never readable for a moment with a wider mode.
pub fn create_partial_with_mode(dir: &Path, name: &str, mode: u32) -> io::Result<(File, Partial)> {
    let name = sanitize_file_name(Some(name));
    for n in 0..MAX_ATTEMPTS {
        let path = dir.join(format!("{}.part", unique_name(&name, n)));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&path)
        {
            Ok(file) => {
                return Ok((
                    file,
                    Partial {
                        path,
                        committed: false,
                    },
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(no_free_name())
}

/// Gives a finished partial its final name in `dir` and returns the path. The name is
/// claimed with `hard_link`, which fails when the target exists: `report.pdf` taken gives
/// `report (1).pdf`, then `report (2).pdf`; an existing file is never replaced or changed.
/// On success the partial name is removed. On failure no final file exists and the partial
/// is removed when `partial` drops. `name` is sanitized again here.
pub fn commit_partial(mut partial: Partial, dir: &Path, name: &str) -> io::Result<PathBuf> {
    let name = sanitize_file_name(Some(name));
    for n in 0..MAX_ATTEMPTS {
        let target = dir.join(unique_name(&name, n));
        match fs::hard_link(&partial.path, &target) {
            Ok(()) => {
                // If removal fails the guard stays armed and retries on drop.
                if fs::remove_file(&partial.path).is_ok() {
                    partial.committed = true;
                }
                return Ok(target);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(no_free_name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        v.sort();
        v
    }

    fn s(name: &str) -> String {
        sanitize_file_name(Some(name))
    }

    #[test]
    fn sanitize_keeps_last_component_only() {
        assert_eq!(s("../../.ssh/x"), "x");
        assert_eq!(s("a/b\\c"), "c");
        assert_eq!(s("C:\\Users\\me\\report.pdf"), "report.pdf");
        assert_eq!(s("/etc/passwd"), "passwd");
        assert_eq!(s("dir/"), FALLBACK_FILE_NAME);
        assert_eq!(s("a/.."), FALLBACK_FILE_NAME);
    }

    #[test]
    fn sanitize_replaces_control_characters_and_colon() {
        assert_eq!(s("a\0b\nc\td\u{7f}e.txt"), "a_b_c_d_e.txt");
        assert_eq!(s("12:30.txt"), "12_30.txt");
    }

    #[test]
    fn sanitize_trims_leading_dots_and_whitespace() {
        assert_eq!(s(".bashrc"), "bashrc");
        assert_eq!(s("..  hidden .txt"), "hidden .txt");
        assert_eq!(s("  report.pdf  "), "report.pdf");
        assert_eq!(s("report.pdf"), "report.pdf");
    }

    #[test]
    fn sanitize_falls_back_for_unusable_names() {
        for name in [
            None,
            Some(""),
            Some("."),
            Some(".."),
            Some("..."),
            Some("  "),
            Some("\u{3000}. ."),
        ] {
            assert_eq!(sanitize_file_name(name), FALLBACK_FILE_NAME, "{name:?}");
        }
    }

    #[test]
    fn sanitize_shortens_long_names_keeping_the_extension() {
        let out = s(&format!("{}.pdf", "a".repeat(300)));
        assert_eq!(out.len(), MAX_NAME_BYTES);
        assert!(out.ends_with(".pdf"));
        assert!(out.starts_with("aaa"));
        // A long "extension" is not kept: the whole name is cut.
        let out = s(&format!("{}.{}", "a".repeat(300), "b".repeat(40)));
        assert_eq!(out.len(), MAX_NAME_BYTES);
        assert!(out.chars().all(|c| c == 'a'));
        assert_eq!(s(&"a".repeat(MAX_NAME_BYTES)).len(), MAX_NAME_BYTES);
    }

    #[test]
    fn sanitize_cuts_multibyte_names_on_char_boundaries() {
        // 3-byte characters: the 196-byte stem budget is not a multiple of 3.
        let out = s(&format!("{}.txt", "日".repeat(100)));
        assert!(out.len() <= MAX_NAME_BYTES);
        let stem = out.strip_suffix(".txt").unwrap();
        assert_eq!(stem, "日".repeat(65));
        // No extension at all.
        let out = s(&"ж".repeat(150));
        assert_eq!(out, "ж".repeat(100));
    }

    #[test]
    fn sanitize_is_idempotent() {
        let long = format!("{}.pdf", "a ".repeat(150));
        for raw in ["../x", "a\nb", &"日".repeat(120), &long, "..", ".a.b"] {
            let once = s(raw);
            assert_eq!(s(&once), once, "{raw:?}");
        }
    }

    #[test]
    fn unique_name_numbers_before_the_extension() {
        assert_eq!(unique_name("report.pdf", 0), "report.pdf");
        assert_eq!(unique_name("report.pdf", 1), "report (1).pdf");
        assert_eq!(unique_name("report.pdf", 12), "report (12).pdf");
        assert_eq!(unique_name("archive.tar.gz", 2), "archive.tar (2).gz");
        assert_eq!(unique_name("Makefile", 1), "Makefile (1)");
        assert_eq!(unique_name("v1.2 final", 1), "v1.2 final (1)");
    }

    #[test]
    fn create_partial_makes_new_files_and_never_reuses_a_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("report.pdf.part"), b"foreign").unwrap();
        let (mut a, pa) = create_partial(dir.path(), "report.pdf").unwrap();
        let (mut b, pb) = create_partial(dir.path(), "report.pdf").unwrap();
        a.write_all(b"A").unwrap();
        b.write_all(b"B").unwrap();
        assert_eq!(pa.path(), dir.path().join("report (1).pdf.part"));
        assert_eq!(pb.path(), dir.path().join("report (2).pdf.part"));
        assert_eq!(
            fs::read(dir.path().join("report.pdf.part")).unwrap(),
            b"foreign"
        );
        assert_eq!(fs::read(pa.path()).unwrap(), b"A");
        assert_eq!(fs::read(pb.path()).unwrap(), b"B");
    }

    #[test]
    fn create_partial_stays_inside_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (_file, partial) = create_partial(dir.path(), "../../.ssh/x").unwrap();
        assert_eq!(partial.path(), dir.path().join("x.part"));
    }

    #[test]
    fn a_private_partial_has_its_mode_from_the_moment_it_exists() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (file, partial) = create_partial_with_mode(dir.path(), "a.bin", 0o600).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(partial.path()), 0o600);
        drop(file);
        let path = commit_partial(partial, dir.path(), "a.bin").unwrap();
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn create_partial_reports_a_missing_folder() {
        let dir = tempfile::tempdir().unwrap();
        let err = create_partial(&dir.path().join("nope"), "a.txt").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn dropping_an_uncommitted_partial_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let (file, partial) = create_partial(dir.path(), "a.txt").unwrap();
        assert_eq!(names(dir.path()), ["a.txt.part"]);
        drop(partial);
        drop(file);
        assert!(names(dir.path()).is_empty());
    }

    #[test]
    fn commit_gives_the_final_name_and_removes_the_partial() {
        let dir = tempfile::tempdir().unwrap();
        let (mut file, partial) = create_partial(dir.path(), "report.pdf").unwrap();
        file.write_all(b"body").unwrap();
        let path = commit_partial(partial, dir.path(), "report.pdf").unwrap();
        assert_eq!(path, dir.path().join("report.pdf"));
        assert_eq!(fs::read(&path).unwrap(), b"body");
        assert_eq!(names(dir.path()), ["report.pdf"]);
    }

    #[test]
    fn commit_never_changes_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let taken = dir.path().join("report.pdf");
        std::fs::write(&taken, b"old").unwrap();
        for (n, expected) in [(1, "report (1).pdf"), (2, "report (2).pdf")] {
            let (mut file, partial) = create_partial(dir.path(), "report.pdf").unwrap();
            file.write_all(format!("new{n}").as_bytes()).unwrap();
            let path = commit_partial(partial, dir.path(), "report.pdf").unwrap();
            assert_eq!(path, dir.path().join(expected));
            assert_eq!(fs::read(&path).unwrap(), format!("new{n}").as_bytes());
            assert_eq!(fs::read(&taken).unwrap(), b"old");
        }
        assert_eq!(
            names(dir.path()),
            ["report (1).pdf", "report (2).pdf", "report.pdf"]
        );
    }

    #[test]
    fn commit_does_not_follow_a_symlink_at_the_final_name() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, b"keep").unwrap();
        std::os::unix::fs::symlink(&victim, dir.path().join("report.pdf")).unwrap();
        let (mut file, partial) = create_partial(dir.path(), "report.pdf").unwrap();
        file.write_all(b"new").unwrap();
        let path = commit_partial(partial, dir.path(), "report.pdf").unwrap();
        assert_eq!(path, dir.path().join("report (1).pdf"));
        assert_eq!(fs::read(&victim).unwrap(), b"keep");
    }

    #[test]
    fn commit_failure_leaves_no_final_file_and_no_partial() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let (mut file, partial) = create_partial(src.path(), "a.txt").unwrap();
        file.write_all(b"x").unwrap();
        let missing = dst.path().join("gone");
        let err = commit_partial(partial, &missing, "a.txt").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(!missing.exists());
        assert!(names(src.path()).is_empty());
        assert!(names(dst.path()).is_empty());
    }

    #[test]
    fn two_downloads_of_one_name_commit_to_distinct_files() {
        let dir = tempfile::tempdir().unwrap();
        let (mut a, pa) = create_partial(dir.path(), "x.bin").unwrap();
        let (mut b, pb) = create_partial(dir.path(), "x.bin").unwrap();
        a.write_all(b"A").unwrap();
        b.write_all(b"B").unwrap();
        let fb = commit_partial(pb, dir.path(), "x.bin").unwrap();
        let fa = commit_partial(pa, dir.path(), "x.bin").unwrap();
        assert_eq!(fs::read(fb).unwrap(), b"B");
        assert_eq!(fs::read(fa).unwrap(), b"A");
        assert_eq!(names(dir.path()), ["x (1).bin", "x.bin"]);
    }
}
