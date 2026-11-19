//! Test helper: captures `tracing` output into a string.

use std::io;
use std::sync::{Arc, Mutex};

use tracing::subscriber::DefaultGuard;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
pub(crate) struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    /// Captures everything down to TRACE for the current thread; use it from a current-thread
    /// runtime (`#[tokio::test]`) so every task runs under it.
    pub fn install(&self) -> DefaultGuard {
        self.install_at(tracing::Level::TRACE)
    }

    pub fn install_at(&self, level: tracing::Level) -> DefaultGuard {
        let subscriber = tracing_subscriber::fmt().with_writer(self.clone()).with_max_level(level).with_ansi(false).finish();
        tracing::subscriber::set_default(subscriber)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    /// Fails when any of `secrets` appears in the captured output.
    pub fn assert_absent(&self, secrets: &[&str]) {
        let text = self.text();
        for secret in secrets {
            assert!(!text.contains(secret), "{secret:?} leaked into the log:\n{text}");
        }
    }
}

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl MakeWriter<'_> for Capture {
    type Writer = Self;

    fn make_writer(&self) -> Self {
        self.clone()
    }
}
