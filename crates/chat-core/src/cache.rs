//! Per-account SQLite cache of chats, history and the media index.
//!
//! Domain values are stored as JSON next to the normalized columns used for ordering and
//! range queries, so a row that no longer deserializes is skipped instead of failing a load.

use crate::model::{
    ChatId, ChatSummary, Draft, HistoryChunk, MediaKind, MediaRef, Message, MessageBody, MessageTs, PayloadId, ReplyTarget,
    UserId,
};
use crate::paths::cache_path;
use rusqlite::{Connection, OptionalExtension, params};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("cache file: {0}")]
    Io(#[from] io::Error),
    #[error("cache database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("cache schema version {found} is newer than the supported {supported}")]
    TooNew { found: i64, supported: i64 },
}

/// Applied in order; `PRAGMA user_version` holds the number already applied.
const MIGRATIONS: &[&str] = &["
    CREATE TABLE chats (
        id TEXT PRIMARY KEY,
        pinned_rank INTEGER,
        last_ts INTEGER,
        json TEXT NOT NULL
    );
    CREATE TABLE messages (
        chat TEXT NOT NULL,
        ts INTEGER NOT NULL,
        json TEXT NOT NULL,
        PRIMARY KEY (chat, ts)
    ) WITHOUT ROWID;
    CREATE TABLE chat_state (
        chat TEXT PRIMARY KEY,
        history_start INTEGER NOT NULL
    );
    CREATE TABLE media (
        kind TEXT NOT NULL,
        id TEXT NOT NULL,
        file TEXT NOT NULL,
        size INTEGER NOT NULL,
        PRIMARY KEY (kind, id)
    );
",
// Pending sends and unsent composer state; both must survive a restart.
"
    CREATE TABLE outbox (
        payload_id TEXT PRIMARY KEY,
        chat TEXT NOT NULL,
        created_us INTEGER NOT NULL,
        json TEXT NOT NULL
    );
    CREATE TABLE drafts (
        chat TEXT PRIMARY KEY,
        json TEXT NOT NULL
    );
"];

/// A send that has not been confirmed yet. `created_us` is the local timestamp of its
/// pending message; `failed` holds the server's reason once it was rejected for good.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OutboxEntry {
    pub payload: PayloadId,
    pub chat: ChatId,
    pub created_us: MessageTs,
    pub text: String,
    pub reply_to: Option<ReplyTarget>,
    pub failed: Option<String>,
}

pub struct Cache {
    conn: Connection,
    /// False for the in-memory fallback: its contents die with the process.
    durable: bool,
}

pub(crate) fn kind_name(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Avatar => "avatar",
        MediaKind::Thumbnail => "thumbnail",
        MediaKind::File => "file",
    }
}

fn encode<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("domain values always serialize")
}

impl Cache {
    /// Opens (creating, mode 0600) `<runtime_dir>/cache/<account>.sqlite` and migrates it.
    pub fn open(runtime_dir: &Path, account: &UserId) -> Result<Self, CacheError> {
        let path = cache_path(runtime_dir, account);
        if let Some(dir) = path.parent() {
            DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        }
        OpenOptions::new().create(true).write(true).truncate(false).mode(0o600).open(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Self::init(Connection::open(&path)?, true)
    }

    pub fn open_in_memory() -> Result<Self, CacheError> {
        Self::init(Connection::open_in_memory()?, false)
    }

    /// Whether what is stored survives a restart.
    pub fn is_durable(&self) -> bool {
        self.durable
    }

    fn init(mut conn: Connection, durable: bool) -> Result<Self, CacheError> {
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let supported = MIGRATIONS.len() as i64;
        if version > supported {
            return Err(CacheError::TooNew { found: version, supported });
        }
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", index as i64 + 1)?;
            tx.commit()?;
        }
        Ok(Self { conn, durable })
    }

    /// Pinned chats first by rank, then newest activity first.
    pub fn chats(&self) -> Result<Vec<ChatSummary>, CacheError> {
        let mut stmt = self.conn.prepare(
            "SELECT json FROM chats
             ORDER BY pinned_rank IS NULL, pinned_rank, last_ts IS NULL, last_ts DESC, id",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.filter_map(|json| serde_json::from_str(&json.ok()?).ok()).collect())
    }

    pub fn chat(&self, id: &ChatId) -> Result<Option<ChatSummary>, CacheError> {
        let json: Option<String> =
            self.conn.query_row("SELECT json FROM chats WHERE id = ?1", [&id.0], |row| row.get(0)).optional()?;
        Ok(json.and_then(|json| serde_json::from_str(&json).ok()))
    }

    /// Makes the cached chat list exactly `chats`; chats that vanished lose their history too.
    pub fn replace_chats(&mut self, chats: &[ChatSummary]) -> Result<(), CacheError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM chats", [])?;
        for chat in chats {
            put_chat(&tx, chat)?;
        }
        tx.execute("DELETE FROM messages WHERE chat NOT IN (SELECT id FROM chats)", [])?;
        tx.execute("DELETE FROM chat_state WHERE chat NOT IN (SELECT id FROM chats)", [])?;
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_chat(&self, chat: &ChatSummary) -> Result<(), CacheError> {
        put_chat(&self.conn, chat)
    }

    /// Cached messages older than `before` (all when `None`), oldest first, at most `limit`.
    /// `reached_start` is set only when the first message of the chat is known to be cached.
    pub fn history(&self, chat: &ChatId, before: Option<MessageTs>, limit: usize) -> Result<HistoryChunk, CacheError> {
        let mut stmt = self
            .conn
            .prepare("SELECT json FROM messages WHERE chat = ?1 AND ts < ?2 ORDER BY ts DESC LIMIT ?3")?;
        let rows = stmt.query_map(params![chat.0, before.unwrap_or(MessageTs::MAX), limit as i64 + 1], |row| {
            row.get::<_, String>(0)
        })?;
        let mut messages: Vec<Message> =
            rows.filter_map(|json| serde_json::from_str(&json.ok()?).ok()).collect();
        let more = messages.len() > limit;
        messages.truncate(limit);
        messages.reverse();
        Ok(HistoryChunk {
            chat: chat.clone(),
            messages,
            reached_start: !more && self.history_start(chat)?,
            replaces: None,
            payloads: Vec::new(),
        })
    }

    /// Cached messages newer than `after` (all when `None`), oldest first.
    pub fn newer_than(&self, chat: &ChatId, after: Option<MessageTs>) -> Result<Vec<Message>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT json FROM messages WHERE chat = ?1 AND ts > ?2 ORDER BY ts")?;
        let rows = stmt.query_map(params![chat.0, after.unwrap_or(MessageTs::MIN)], |row| row.get::<_, String>(0))?;
        Ok(rows.filter_map(|json| serde_json::from_str(&json.ok()?).ok()).collect())
    }

    /// Cached messages with `lo <= ts <= hi`, oldest first.
    pub fn messages_between(&self, chat: &ChatId, lo: MessageTs, hi: MessageTs) -> Result<Vec<Message>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT json FROM messages WHERE chat = ?1 AND ts >= ?2 AND ts <= ?3 ORDER BY ts")?;
        let rows = stmt.query_map(params![chat.0, lo, hi], |row| row.get::<_, String>(0))?;
        Ok(rows.filter_map(|json| serde_json::from_str(&json.ok()?).ok()).collect())
    }

    pub fn message(&self, chat: &ChatId, ts: MessageTs) -> Result<Option<Message>, CacheError> {
        let json: Option<String> = self
            .conn
            .query_row("SELECT json FROM messages WHERE chat = ?1 AND ts = ?2", params![chat.0, ts], |row| row.get(0))
            .optional()?;
        Ok(json.and_then(|json| serde_json::from_str(&json).ok()))
    }

    pub fn newest_ts(&self, chat: &ChatId) -> Result<Option<MessageTs>, CacheError> {
        Ok(self.conn.query_row("SELECT MAX(ts) FROM messages WHERE chat = ?1", [&chat.0], |row| row.get(0))?)
    }

    pub fn history_start(&self, chat: &ChatId) -> Result<bool, CacheError> {
        let flag: Option<i64> = self
            .conn
            .query_row("SELECT history_start FROM chat_state WHERE chat = ?1", [&chat.0], |row| row.get(0))
            .optional()?;
        Ok(flag == Some(1))
    }

    pub fn chats_with_messages(&self) -> Result<Vec<ChatId>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT DISTINCT chat FROM messages")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?.into_iter().map(ChatId).collect())
    }

    /// Server data for the time range `lo..=hi` (`None` = unbounded) replaces what is cached
    /// there, so messages the server no longer returns disappear. `history_start`, when given,
    /// records whether the first message of the chat is now cached.
    pub fn replace_range(
        &mut self,
        chat: &ChatId,
        lo: Option<MessageTs>,
        hi: Option<MessageTs>,
        messages: &[Message],
        history_start: Option<bool>,
    ) -> Result<(), CacheError> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM messages WHERE chat = ?1 AND ts >= ?2 AND ts <= ?3",
            params![chat.0, lo.unwrap_or(MessageTs::MIN), hi.unwrap_or(MessageTs::MAX)],
        )?;
        for message in messages {
            put_message(&tx, chat, message)?;
        }
        if let Some(flag) = history_start {
            tx.execute(
                "INSERT OR REPLACE INTO chat_state (chat, history_start) VALUES (?1, ?2)",
                params![chat.0, flag as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_message(&self, message: &Message) -> Result<(), CacheError> {
        put_message(&self.conn, &message.chat, message)
    }

    /// Turns a cached message into the deleted placeholder; false when it is not cached.
    pub fn mark_deleted(&self, chat: &ChatId, ts: MessageTs) -> Result<bool, CacheError> {
        let Some(mut message) = self.message(chat, ts)? else { return Ok(false) };
        message.body = MessageBody::Deleted;
        put_message(&self.conn, chat, &message)?;
        Ok(true)
    }

    /// File name (inside the account media dir) and size of a downloaded media item.
    pub fn media(&self, media: &MediaRef) -> Result<Option<(String, u64)>, CacheError> {
        Ok(self
            .conn
            .query_row(
                "SELECT file, size FROM media WHERE kind = ?1 AND id = ?2",
                params![kind_name(media.kind), media.id],
                |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)),
            )
            .optional()?)
    }

    pub fn put_media(&self, media: &MediaRef, file: &str, size: u64) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO media (kind, id, file, size) VALUES (?1, ?2, ?3, ?4)",
            params![kind_name(media.kind), media.id, file, size as i64],
        )?;
        Ok(())
    }

    /// Unconfirmed sends in the order they were queued.
    pub fn outbox(&self) -> Result<Vec<OutboxEntry>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT json FROM outbox ORDER BY rowid")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.filter_map(|json| serde_json::from_str(&json.ok()?).ok()).collect())
    }

    /// Queues a send and drops the chat's draft in one transaction: either both happen or
    /// neither, so a failed write never loses the draft or leaves an unstored send.
    pub fn enqueue(&self, entry: &OutboxEntry) -> Result<(), CacheError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO outbox (payload_id, chat, created_us, json) VALUES (?1, ?2, ?3, ?4)",
            params![entry.payload.0, entry.chat.0, entry.created_us, encode(entry)],
        )?;
        tx.execute("DELETE FROM drafts WHERE chat = ?1", [&entry.chat.0])?;
        Ok(tx.commit()?)
    }

    /// Inserts or updates an entry; an update keeps its queue position.
    pub fn put_outbox(&self, entry: &OutboxEntry) -> Result<(), CacheError> {
        self.conn.execute(
            "INSERT INTO outbox (payload_id, chat, created_us, json) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (payload_id) DO UPDATE SET json = excluded.json",
            params![entry.payload.0, entry.chat.0, entry.created_us, encode(entry)],
        )?;
        Ok(())
    }

    pub fn remove_outbox(&self, payload: &PayloadId) -> Result<(), CacheError> {
        self.conn.execute("DELETE FROM outbox WHERE payload_id = ?1", [&payload.0])?;
        Ok(())
    }

    pub fn drafts(&self) -> Result<Vec<Draft>, CacheError> {
        let mut stmt = self.conn.prepare("SELECT json FROM drafts ORDER BY chat")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.filter_map(|json| serde_json::from_str(&json.ok()?).ok()).collect())
    }

    pub fn put_draft(&self, draft: &Draft) -> Result<(), CacheError> {
        self.conn
            .execute("INSERT OR REPLACE INTO drafts (chat, json) VALUES (?1, ?2)", params![draft.chat.0, encode(draft)])?;
        Ok(())
    }

    pub fn remove_draft(&self, chat: &ChatId) -> Result<(), CacheError> {
        self.conn.execute("DELETE FROM drafts WHERE chat = ?1", [&chat.0])?;
        Ok(())
    }
}

fn put_chat(conn: &Connection, chat: &ChatSummary) -> Result<(), CacheError> {
    conn.execute(
        "INSERT OR REPLACE INTO chats (id, pinned_rank, last_ts, json) VALUES (?1, ?2, ?3, ?4)",
        params![chat.id.0, chat.pinned_rank, chat.last_message.as_ref().map(|m| m.timestamp), encode(chat)],
    )?;
    Ok(())
}

fn put_message(conn: &Connection, chat: &ChatId, message: &Message) -> Result<(), CacheError> {
    conn.execute(
        "INSERT OR REPLACE INTO messages (chat, ts, json) VALUES (?1, ?2, ?3)",
        params![chat.0, message.timestamp, encode(message)],
    )?;
    Ok(())
}

/// Deletes the account's cache database, its journal files and the `.broken-*` copies (with
/// their sidecars) that `move_aside` kept: they hold the same history and drafts, so an
/// explicit sign-out must not leave them behind.
pub fn remove_files(runtime_dir: &Path, account: &UserId) -> io::Result<()> {
    let path = cache_path(runtime_dir, account);
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let mut name = path.clone().into_os_string();
        name.push(suffix);
        match fs::remove_file(&name) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    let (Some(dir), Some(file)) = (path.parent(), path.file_name()) else { return Ok(()) };
    let mut backup_prefix = file.to_os_string();
    backup_prefix.push(".broken-");
    let backup_prefix = backup_prefix.as_encoded_bytes();
    let entries = match fs::read_dir(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        entries => entries?,
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_name().as_encoded_bytes().starts_with(backup_prefix) {
            match fs::remove_file(entry.path()) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
    }
    Ok(())
}

/// Moves the account's database and journal files aside instead of deleting them: a cache
/// that cannot be opened may still hold the only copy of unsent messages and drafts, and a
/// sidecar kept next to its database lets SQLite recover it later. Returns the new path of
/// the main file; an existing `.broken-*` file is never overwritten.
pub fn move_aside(runtime_dir: &Path, account: &UserId, unix_secs: u64) -> io::Result<PathBuf> {
    let path = cache_path(runtime_dir, account);
    let with_suffix = |base: &Path, suffix: &str| {
        let mut name = base.to_path_buf().into_os_string();
        name.push(suffix);
        PathBuf::from(name)
    };
    let target = (0u32..)
        .map(|n| with_suffix(&path, &if n == 0 { format!(".broken-{unix_secs}") } else { format!(".broken-{unix_secs}-{n}") }))
        .find(|target| !target.exists())
        .expect("a free name exists");
    for suffix in ["", "-journal", "-wal", "-shm"] {
        match fs::rename(with_suffix(&path, suffix), with_suffix(&target, suffix)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{chat, id, msg};
    use crate::model::Delivery;

    fn texts(chunk: &HistoryChunk) -> Vec<(MessageTs, String)> {
        chunk
            .messages
            .iter()
            .map(|m| match &m.body {
                MessageBody::Text(t) => (m.timestamp, t.clone()),
                other => (m.timestamp, format!("{other:?}")),
            })
            .collect()
    }

    #[test]
    fn cold_load_survives_reopen_with_private_mode() {
        let dir = tempfile::tempdir().unwrap();
        let account = UserId("42".into());
        {
            let mut cache = Cache::open(dir.path(), &account).unwrap();
            cache.replace_chats(&[chat("a", Some(5), None), chat("b", Some(9), None), chat("p", Some(1), Some(0))]).unwrap();
            cache.replace_range(&id("a"), None, None, &[msg("a", 1, "one"), msg("a", 2, "two")], Some(true)).unwrap();
        }
        let path = cache_path(dir.path(), &account);
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

        let cache = Cache::open(dir.path(), &account).unwrap();
        let order: Vec<_> = cache.chats().unwrap().into_iter().map(|c| c.id.0).collect();
        assert_eq!(order, ["p", "b", "a"]);
        let chunk = cache.history(&id("a"), None, 50).unwrap();
        assert_eq!(texts(&chunk), [(1, "one".into()), (2, "two".into())]);
        assert!(chunk.reached_start);
        let version: i64 = cache.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
    }

    #[test]
    fn history_pages_and_reports_start_only_when_all_cached() {
        let mut cache = Cache::open_in_memory().unwrap();
        let all: Vec<_> = (1..=5).map(|i| msg("a", i, "m")).collect();
        cache.replace_range(&id("a"), None, None, &all, Some(true)).unwrap();
        let newest = cache.history(&id("a"), None, 3).unwrap();
        assert_eq!(newest.messages.iter().map(|m| m.timestamp).collect::<Vec<_>>(), [3, 4, 5]);
        assert!(!newest.reached_start);
        let older = cache.history(&id("a"), Some(3), 3).unwrap();
        assert_eq!(older.messages.iter().map(|m| m.timestamp).collect::<Vec<_>>(), [1, 2]);
        assert!(older.reached_start);
    }

    #[test]
    fn range_replace_drops_server_deleted_messages() {
        let mut cache = Cache::open_in_memory().unwrap();
        let a = id("a");
        let all: Vec<_> = (1..=6).map(|i| msg("a", i, "old")).collect();
        cache.replace_range(&a, None, None, &all, None).unwrap();
        // Server now returns 3 and 5 for the range 3..=5 (4 vanished) with an edit of 5.
        cache.replace_range(&a, Some(3), Some(5), &[msg("a", 3, "old"), msg("a", 5, "new")], None).unwrap();
        let chunk = cache.history(&a, None, 50).unwrap();
        assert_eq!(
            texts(&chunk),
            [(1, "old".into()), (2, "old".into()), (3, "old".into()), (5, "new".into()), (6, "old".into())]
        );
    }

    #[test]
    fn range_replace_is_scoped_to_its_chat() {
        let mut cache = Cache::open_in_memory().unwrap();
        cache.replace_range(&id("a"), None, None, &[msg("a", 1, "a")], None).unwrap();
        cache.replace_range(&id("b"), None, None, &[msg("b", 1, "b")], None).unwrap();
        cache.replace_range(&id("a"), None, None, &[], None).unwrap();
        assert!(cache.message(&id("a"), 1).unwrap().is_none());
        assert!(cache.message(&id("b"), 1).unwrap().is_some());
    }

    #[test]
    fn mark_deleted_keeps_a_placeholder() {
        let cache = Cache::open_in_memory().unwrap();
        cache.upsert_message(&msg("a", 1, "secret")).unwrap();
        assert!(cache.mark_deleted(&id("a"), 1).unwrap());
        assert_eq!(cache.message(&id("a"), 1).unwrap().unwrap().body, MessageBody::Deleted);
        assert!(!cache.mark_deleted(&id("a"), 2).unwrap());
    }

    #[test]
    fn a_row_written_before_reactions_existed_loads_without_them() {
        let cache = Cache::open_in_memory().unwrap();
        let mut json: serde_json::Value = serde_json::from_str(&encode(&msg("a", 1, "old"))).unwrap();
        assert!(json.as_object_mut().unwrap().remove("reactions").is_some(), "the field is serialized today");
        cache.conn.execute("INSERT INTO messages (chat, ts, json) VALUES ('a', 1, ?1)", [json.to_string()]).unwrap();
        let loaded = cache.message(&id("a"), 1).unwrap().unwrap();
        assert_eq!(loaded.reactions, Default::default());
        assert_eq!(cache.history(&id("a"), None, 50).unwrap().messages[0].reactions, Default::default());
    }

    #[test]
    fn messages_between_is_inclusive_ordered_and_scoped_to_its_chat() {
        let cache = Cache::open_in_memory().unwrap();
        for ts in [4, 1, 3, 2] {
            cache.upsert_message(&msg("a", ts, "x")).unwrap();
        }
        cache.upsert_message(&msg("b", 2, "other")).unwrap();
        let stamps: Vec<_> = cache.messages_between(&id("a"), 2, 3).unwrap().iter().map(|m| m.timestamp).collect();
        assert_eq!(stamps, [2, 3]);
        assert!(cache.messages_between(&id("a"), 5, 9).unwrap().is_empty());
    }

    #[test]
    fn replacing_chats_drops_history_of_removed_chats() {
        let mut cache = Cache::open_in_memory().unwrap();
        cache.replace_chats(&[chat("a", None, None), chat("b", None, None)]).unwrap();
        cache.replace_range(&id("a"), None, None, &[msg("a", 1, "x")], Some(true)).unwrap();
        cache.replace_chats(&[chat("b", None, None)]).unwrap();
        assert!(cache.message(&id("a"), 1).unwrap().is_none());
        assert!(!cache.history_start(&id("a")).unwrap());
        assert!(cache.chats_with_messages().unwrap().is_empty());
    }

    #[test]
    fn accounts_do_not_share_data() {
        let dir = tempfile::tempdir().unwrap();
        let first = UserId("1".into());
        let second = UserId("2".into());
        let mut cache = Cache::open(dir.path(), &first).unwrap();
        cache.replace_chats(&[chat("a", Some(1), None)]).unwrap();
        cache.upsert_message(&msg("a", 1, "private")).unwrap();
        cache.put_media(&MediaRef { id: "m".into(), kind: MediaKind::Avatar }, "f", 3).unwrap();

        let other = Cache::open(dir.path(), &second).unwrap();
        assert!(other.chats().unwrap().is_empty());
        assert!(other.history(&id("a"), None, 50).unwrap().messages.is_empty());
        assert!(other.media(&MediaRef { id: "m".into(), kind: MediaKind::Avatar }).unwrap().is_none());
        assert_ne!(cache_path(dir.path(), &first), cache_path(dir.path(), &second));
    }

    #[test]
    fn newer_schema_is_rejected_and_remove_files_deletes_everything() {
        let dir = tempfile::tempdir().unwrap();
        let account = UserId("7".into());
        let path = cache_path(dir.path(), &account);
        drop(Cache::open(dir.path(), &account).unwrap());
        Connection::open(&path).unwrap().pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(Cache::open(dir.path(), &account), Err(CacheError::TooNew { found: 99, .. })));
        remove_files(dir.path(), &account).unwrap();
        assert!(!path.exists());
        remove_files(dir.path(), &account).unwrap();
    }

    #[test]
    fn media_index_round_trips_per_kind() {
        let cache = Cache::open_in_memory().unwrap();
        let avatar = MediaRef { id: "x".into(), kind: MediaKind::Avatar };
        let thumb = MediaRef { id: "x".into(), kind: MediaKind::Thumbnail };
        cache.put_media(&avatar, "avatar-x", 10).unwrap();
        assert_eq!(cache.media(&avatar).unwrap(), Some(("avatar-x".into(), 10)));
        assert_eq!(cache.media(&thumb).unwrap(), None);
    }

    fn entry(payload: &str, chat: &str, created_us: MessageTs, text: &str) -> OutboxEntry {
        OutboxEntry { payload: PayloadId(payload.into()), chat: id(chat), created_us, text: text.into(), reply_to: None, failed: None }
    }

    #[test]
    fn version_one_database_upgrades_in_place_and_keeps_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let account = UserId("9".into());
        let path = cache_path(dir.path(), &account);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            put_chat(&conn, &chat("a", Some(2), None)).unwrap();
            put_message(&conn, &id("a"), &msg("a", 2, "kept")).unwrap();
            // A version-1 message row has no `delivery` field.
            let mut json: serde_json::Value = serde_json::from_str(&encode(&msg("a", 1, "old"))).unwrap();
            json.as_object_mut().unwrap().remove("delivery");
            conn.execute("INSERT INTO messages (chat, ts, json) VALUES ('a', 1, ?1)", [json.to_string()]).unwrap();
        }

        let cache = Cache::open(dir.path(), &account).unwrap();
        let version: i64 = cache.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(version, 2);
        assert_eq!(cache.chats().unwrap().len(), 1);
        let chunk = cache.history(&id("a"), None, 50).unwrap();
        assert_eq!(texts(&chunk), [(1, "old".into()), (2, "kept".into())]);
        assert_eq!(chunk.messages[0].delivery, Delivery::Sent);
        assert!(cache.outbox().unwrap().is_empty());
        assert!(cache.drafts().unwrap().is_empty());
        cache.put_outbox(&entry("p", "a", 3, "new")).unwrap();
        cache.put_draft(&Draft { chat: id("a"), text: "d".into(), reply_to: None }).unwrap();
        assert_eq!(cache.outbox().unwrap().len(), 1);
        assert_eq!(cache.drafts().unwrap().len(), 1);
    }

    #[test]
    fn outbox_keeps_queue_order_through_updates_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let account = UserId("5".into());
        {
            let cache = Cache::open(dir.path(), &account).unwrap();
            cache.put_outbox(&entry("p1", "a", 30, "first")).unwrap();
            cache.put_outbox(&entry("p2", "b", 20, "second")).unwrap();
            let mut failed = entry("p1", "a", 30, "first");
            failed.failed = Some("NO_PERMISSION".into());
            cache.put_outbox(&failed).unwrap();
        }
        let cache = Cache::open(dir.path(), &account).unwrap();
        let entries = cache.outbox().unwrap();
        assert_eq!(entries.iter().map(|e| e.payload.0.as_str()).collect::<Vec<_>>(), ["p1", "p2"]);
        assert_eq!(entries[0].failed.as_deref(), Some("NO_PERMISSION"));
        cache.remove_outbox(&PayloadId("p1".into())).unwrap();
        assert_eq!(cache.outbox().unwrap().len(), 1);
    }

    #[test]
    fn drafts_are_replaced_per_chat_and_removed() {
        let cache = Cache::open_in_memory().unwrap();
        let reply = ReplyTarget { timestamp: 4, author: Some("Ann".into()), text: "hi".into() };
        cache.put_draft(&Draft { chat: id("a"), text: "one".into(), reply_to: None }).unwrap();
        cache.put_draft(&Draft { chat: id("a"), text: "two".into(), reply_to: Some(reply.clone()) }).unwrap();
        cache.put_draft(&Draft { chat: id("b"), text: "other".into(), reply_to: None }).unwrap();
        let drafts = cache.drafts().unwrap();
        assert_eq!(drafts.len(), 2);
        assert_eq!((drafts[0].text.as_str(), drafts[0].reply_to.clone()), ("two", Some(reply)));
        cache.remove_draft(&id("a")).unwrap();
        assert_eq!(cache.drafts().unwrap().len(), 1);
    }

    #[test]
    fn enqueue_is_atomic_a_failed_insert_keeps_the_draft() {
        let cache = Cache::open_in_memory().unwrap();
        let draft = Draft { chat: id("a"), text: "typed".into(), reply_to: None };
        cache.put_draft(&draft).unwrap();
        cache.enqueue(&entry("p1", "b", 1, "ok")).unwrap();
        cache.conn.execute_batch("CREATE TRIGGER refuse BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT, 'full'); END;").unwrap();
        assert!(cache.enqueue(&entry("p2", "a", 2, "refused")).is_err());
        assert_eq!(cache.drafts().unwrap(), [draft]);
        assert_eq!(cache.outbox().unwrap().len(), 1);

        cache.conn.execute_batch("DROP TRIGGER refuse").unwrap();
        cache.enqueue(&entry("p3", "a", 3, "ok")).unwrap();
        assert!(cache.drafts().unwrap().is_empty());
        assert_eq!(cache.outbox().unwrap().len(), 2);
    }

    #[test]
    fn move_aside_keeps_every_file_with_its_mode_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let account = UserId("3".into());
        let path = cache_path(dir.path(), &account);
        let named = |suffix: &str| {
            let mut name = path.clone().into_os_string();
            name.push(suffix);
            PathBuf::from(name)
        };
        drop(Cache::open(dir.path(), &account).unwrap());
        fs::write(named("-wal"), b"wal").unwrap();

        let first = move_aside(dir.path(), &account, 1700).unwrap();
        assert_eq!(first, named(".broken-1700"));
        assert!(!path.exists() && !named("-wal").exists());
        assert_eq!(fs::read(named(".broken-1700-wal")).unwrap(), b"wal");
        assert_eq!(fs::metadata(&first).unwrap().permissions().mode() & 0o777, 0o600);

        // A repeat in the same second keeps both.
        fs::write(&path, b"second").unwrap();
        let second = move_aside(dir.path(), &account, 1700).unwrap();
        assert_eq!(second, named(".broken-1700-1"));
        assert_eq!(fs::read(&second).unwrap(), b"second");
        assert!(first.exists());
    }

    #[test]
    fn remove_files_also_deletes_the_accounts_moved_aside_backups_and_only_those() {
        let dir = tempfile::tempdir().unwrap();
        let (mine, other, prefix) = (UserId("3".into()), UserId("33".into()), UserId("2".into()));
        let named = |account: &UserId, suffix: &str| {
            let mut name = cache_path(dir.path(), account).into_os_string();
            name.push(suffix);
            PathBuf::from(name)
        };
        drop(Cache::open(dir.path(), &mine).unwrap());
        fs::write(named(&mine, "-wal"), b"wal").unwrap();
        let first = move_aside(dir.path(), &mine, 1700).unwrap();
        fs::write(cache_path(dir.path(), &mine), b"x").unwrap();
        let second = move_aside(dir.path(), &mine, 1700).unwrap();
        for suffix in ["-journal", "-wal", "-shm"] {
            fs::write(named(&mine, &format!(".broken-1700-1{suffix}")), b"s").unwrap();
        }
        let kept = [named(&other, ""), named(&other, ".broken-5"), named(&prefix, ".broken-5"), named(&other, ".broken-5-wal")];
        for path in &kept {
            fs::write(path, b"other").unwrap();
        }
        assert!(first.exists() && second.exists());

        remove_files(dir.path(), &mine).unwrap();

        let left: Vec<_> = fs::read_dir(cache_path(dir.path(), &mine).parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("3.sqlite"))
            .collect();
        assert!(left.is_empty(), "this account's files survive sign-out: {left:?}");
        assert!(kept.iter().all(|path| path.exists()), "another account's files were deleted");
    }
}
