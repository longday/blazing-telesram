use std::path::{Path, PathBuf};

pub const RUNTIME_DIR_VAR: &str = "BT_RUNTIME_DIR";

/// Root of all local state: session, cache and media.
#[derive(Clone, Debug)]
pub struct RuntimeDir(PathBuf);

#[derive(Debug, thiserror::Error)]
#[error(
    "{RUNTIME_DIR_VAR} is not set: point it at the directory for session and cache files (the run-*.sh and run-windows.ps1 scripts use <repo>/.runtime)"
)]
pub struct MissingRuntimeDir;

impl RuntimeDir {
    pub fn from_env() -> Result<Self, MissingRuntimeDir> {
        match std::env::var_os(RUNTIME_DIR_VAR) {
            Some(path) if !path.is_empty() => Ok(Self(path.into())),
            _ => Err(MissingRuntimeDir),
        }
    }

    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn session_file(&self) -> PathBuf {
        self.0.join("session.json")
    }
}
