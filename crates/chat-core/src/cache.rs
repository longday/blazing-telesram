//! Per-account SQLite cache of chats, history and the media index.
//!
//! Domain values are stored as JSON next to the normalized columns used for ordering and
//! range queries, so a row that no longer deserializes is skipped instead of failing a load.

use crate::model::{ChatId, ChatSummary, HistoryChunk, MediaKind, MediaRef, Message, MessageBody, MessageTs, UserId};
use crate::paths::cache_path;
use rusqlite::{Connection, OptionalExtension, params};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

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
"];

pub struct Cache {
    conn: Connection,
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
        Self::init(Connection::open(&path)?)
    }

    pub fn open_in_memory() -> Result<Self, CacheError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self, CacheError> {
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
        Ok(Self { conn })
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
        Ok(HistoryChunk { chat: chat.clone(), messages, reached_start: !more && self.history_start(chat)? })
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

/// Deletes the account's cache database and its journal files.
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{chat, id, msg};

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
}
