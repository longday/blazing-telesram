//! Diagnostic log: one `BT_LOG` filter (see `yandex_proto::log`) feeding stderr, a JSON Lines
//! file under the runtime directory and an in-memory ring shown by the log window.

use crate::format::{Clock, utc_stamp};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, IsTerminal};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, fmt};

const LOG_DIR: &str = "logs";
const LOG_EXT: &str = "jsonl";
/// Files kept after startup, the new one included.
const KEEP_FILES: usize = 10;
pub const RING_CAPACITY: usize = 2000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub time_us: i64,
    pub level: Level,
    pub target: String,
    /// Message followed by ` name=value` fields.
    pub text: String,
}

#[derive(Default)]
struct Ring {
    lines: VecDeque<LogLine>,
    generation: u64,
}

/// The last [`RING_CAPACITY`] records; a GPUI global so the chat window can show them.
#[derive(Clone, Default)]
pub struct LogBuffer(Arc<Mutex<Ring>>);

impl gpui::Global for LogBuffer {}

impl LogBuffer {
    fn ring(&self) -> MutexGuard<'_, Ring> {
        // A panic while holding the lock leaves a consistent ring: every update is one push/pop.
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Changes whenever a record is added.
    pub fn generation(&self) -> u64 {
        self.ring().generation
    }

    pub fn snapshot(&self) -> Vec<LogLine> {
        self.ring().lines.iter().cloned().collect()
    }

    pub fn push(&self, line: LogLine) {
        let mut ring = self.ring();
        if ring.lines.len() == RING_CAPACITY {
            ring.lines.pop_front();
        }
        ring.lines.push_back(line);
        ring.generation += 1;
    }
}

/// Collects an event's message and fields into one line. Records bridged from the `log`
/// crate carry their real target in `log.target`.
#[derive(Default)]
struct LineVisitor {
    message: String,
    fields: String,
    log_target: Option<String>,
}

impl LineVisitor {
    fn put(&mut self, field: &Field, value: &dyn std::fmt::Display) {
        match field.name() {
            "message" => self.message = value.to_string(),
            "log.target" => self.log_target = Some(value.to_string()),
            name if name.starts_with("log.") => {}
            name => {
                let _ = write!(self.fields, " {name}={value}");
            }
        }
    }
}

impl Visit for LineVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.put(field, &value);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.put(field, &format_args!("{value:?}"));
    }
}

impl<S: Subscriber> Layer<S> for LogBuffer {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut visitor = LineVisitor::default();
        event.record(&mut visitor);
        let meta = event.metadata();
        self.push(LogLine {
            time_us: Clock::system().now_us(),
            level: *meta.level(),
            target: visitor
                .log_target
                .unwrap_or_else(|| meta.target().to_owned()),
            text: visitor.message + &visitor.fields,
        });
    }
}

/// Creates `<runtime>/logs/<UTC stamp>-<pid>.jsonl` (owner-only), first deleting the oldest
/// files so that [`KEEP_FILES`] remain afterwards.
pub fn open_log_file(runtime: &Path, now_us: i64) -> io::Result<(File, PathBuf)> {
    let dir = runtime.join(LOG_DIR);
    DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let mut old: Vec<PathBuf> = fs::read_dir(&dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == LOG_EXT))
        .collect();
    // Names start with the UTC stamp, so name order is age order.
    old.sort();
    let excess = (old.len() + 1).saturating_sub(KEEP_FILES);
    for path in &old[..excess] {
        fs::remove_file(path)?;
    }
    let path = dir.join(format!(
        "{}-{}.{LOG_EXT}",
        utc_stamp(now_us),
        std::process::id()
    ));
    let file = OpenOptions::new()
        .append(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    Ok((file, path))
}

/// One JSON object per record; the unbuffered file hands every record to the OS at once.
fn file_layer<S>(file: File) -> impl Layer<S>
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fmt::layer().json().with_writer(Mutex::new(file))
}

/// Installs the global logger, `log` crate bridge included. Fails only on an invalid `BT_LOG`;
/// a log file that cannot be created is reported through the other outputs.
pub fn init(runtime: &Path) -> Result<LogBuffer, String> {
    let filter = yandex_proto::log::filter_from_env()?;
    let buffer = LogBuffer::default();
    let (file, file_result) = match open_log_file(runtime, Clock::system().now_us()) {
        Ok((file, path)) => (Some(file), Ok(path)),
        Err(error) => (None, Err(error)),
    };
    install(filter, file, buffer.clone())?;
    match file_result {
        Ok(path) => tracing::info!(path = %path.display(), "logging to file"),
        Err(error) => {
            tracing::warn!(%error, "cannot create the log file; logging to stderr and the log window only")
        }
    }
    Ok(buffer)
}

fn install(filter: EnvFilter, file: Option<File>, buffer: LogBuffer) -> Result<(), String> {
    tracing_subscriber::registry()
        .with(filter)
        .with(yandex_proto::log::dependency_cap())
        .with(
            fmt::layer()
                .with_writer(io::stderr)
                .with_ansi(io::stderr().is_terminal()),
        )
        .with(file.map(file_layer))
        .with(buffer)
        .try_init()
        .map_err(|e| format!("cannot install the logger: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    /// The only test that installs the global logger: the `log` bridge needs it. A real
    /// WebSocket handshake with tungstenite, which logs the whole request (`Cookie` included)
    /// at `trace`, under `BT_LOG=trace`.
    #[test]
    fn trace_level_keeps_dependency_secrets_out() {
        use futures::{SinkExt, StreamExt};
        use yandex_proto::ws::TungsteniteConnector;
        use yandex_proto::xiva::{Connector, Socket, Target};

        const COOKIE: &str = "Session_id=fake-secret-4711";
        let buffer = LogBuffer::default();
        install(
            yandex_proto::log::parse_filter(Some("trace")).unwrap(),
            None,
            buffer.clone(),
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                ws.send(tokio_tungstenite::tungstenite::Message::Close(None))
                    .await
                    .unwrap();
                while ws.next().await.is_some() {}
            });
            let target = Target {
                url: format!("ws://{addr}/v2?session=fake-query-0815"),
                cookie: COOKIE.into(),
                origin: "https://yandex.ru".into(),
            };
            let mut socket = TungsteniteConnector::default()
                .connect(&target)
                .await
                .unwrap();
            socket.close().await;
            server.await.unwrap();
        });
        let lines = buffer.snapshot();
        let text: String = lines
            .iter()
            .map(|l| format!("{} {}\n", l.target, l.text))
            .collect();
        assert!(
            lines
                .iter()
                .any(|l| l.target.starts_with("tungstenite") && l.level == Level::DEBUG),
            "the log bridge delivers tungstenite records:\n{text}"
        );
        for secret in ["fake-secret-4711", "fake-query-0815"] {
            assert!(!text.contains(secret), "{secret} leaked:\n{text}");
        }
    }

    #[test]
    fn ring_keeps_the_newest_records() {
        let buffer = LogBuffer::default();
        for i in 0..RING_CAPACITY + 5 {
            buffer.push(LogLine {
                time_us: i as i64,
                level: Level::INFO,
                target: "t".into(),
                text: i.to_string(),
            });
        }
        let lines = buffer.snapshot();
        assert_eq!(lines.len(), RING_CAPACITY);
        assert_eq!(lines[0].text, "5");
        assert_eq!(buffer.generation(), (RING_CAPACITY + 5) as u64);
    }

    #[test]
    fn layer_renders_message_fields_and_log_target() {
        let buffer = LogBuffer::default();
        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(buffer.clone()),
            || {
                tracing::warn!(target: "yandex_proto::xiva", chats = 3, reason = "closed", "disconnected");
            },
        );
        let lines = buffer.snapshot();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].level, Level::WARN);
        assert_eq!(lines[0].target, "yandex_proto::xiva");
        assert_eq!(lines[0].text, "disconnected chats=3 reason=closed");
    }

    #[test]
    fn rotation_keeps_ten_owner_only_files() {
        let runtime = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for i in 0..KEEP_FILES as i64 + 2 {
            let (_, path) = open_log_file(runtime.path(), i * 1_000_000).unwrap();
            paths.push(path);
        }
        let dir = runtime.path().join(LOG_DIR);
        let mut left: Vec<PathBuf> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        left.sort();
        assert_eq!(left, paths[2..]);
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&left[0]).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn file_records_are_json_lines_readable_while_open() {
        let runtime = tempfile::tempdir().unwrap();
        let (file, path) = open_log_file(runtime.path(), 0).unwrap();
        let subscriber = tracing_subscriber::registry().with(file_layer(file));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(status = 200, "history loaded");
            // Read before the writer is dropped: the record must already be on disk.
            let text = fs::read_to_string(&path).unwrap();
            let record: serde_json::Value =
                serde_json::from_str(text.lines().next().unwrap()).unwrap();
            for key in ["timestamp", "level", "target", "fields"] {
                assert!(record.get(key).is_some(), "{key} missing in {record}");
            }
            assert_eq!(record["fields"]["message"], "history loaded");
            assert_eq!(record["fields"]["status"], 200);
        });
    }
}
