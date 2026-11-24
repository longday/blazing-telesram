//! Facade over the socket, registry and media clients: reads, and the closed write set of
//! [`WriteRequest`]. This is what the adapter in `bt-chat-core` maps to its `Backend` trait.

use std::io::Write;

use crate::bootstrap::RegistryEndpoints;
use crate::error::ProtoError;
use crate::media::{MediaClient, MediaConfig, MediaTarget};
use crate::net::HttpSettings;
use crate::registry::RegistryClient;
use crate::request::{HISTORY_CHUNK, SEARCH_LIMIT, SearchEntity};
use crate::search::SearchReply;
use crate::session::Session;
use crate::wire::{DEFAULT_EXCLUDED_NAMESPACES, WireChat, WireHistory, WireUser};
use crate::write::{Committed, WriteRequest};
use crate::ws::TungsteniteConnector;
use crate::xiva::{Connector, XivaConfig, XivaEvents, XivaHandle};

#[derive(Clone, Debug)]
pub struct ClientConfig {
    pub xiva: XivaConfig,
    pub registry: RegistryEndpoints,
    pub media: MediaConfig,
    pub http: HttpSettings,
    pub excluded_namespaces: Vec<u32>,
    pub history_limit: u32,
}

impl ClientConfig {
    pub fn production() -> Self {
        Self {
            xiva: XivaConfig::production(),
            registry: RegistryEndpoints::production(),
            media: MediaConfig::production(),
            http: HttpSettings::default(),
            excluded_namespaces: DEFAULT_EXCLUDED_NAMESPACES.to_vec(),
            history_limit: HISTORY_CHUNK,
        }
    }
}

#[derive(Clone)]
pub struct Client {
    xiva: XivaHandle,
    registry: RegistryClient,
    media: MediaClient,
    excluded_namespaces: Vec<u32>,
    history_limit: u32,
}

impl Client {
    /// Connects to production Xiva. Must run inside a Tokio runtime; the returned stream
    /// carries connection state and pushes.
    pub fn start(session: &Session, config: ClientConfig) -> Result<(Self, XivaEvents), ProtoError> {
        Self::start_with(TungsteniteConnector::default(), session, config)
    }

    pub fn start_with<C: Connector>(
        connector: C,
        session: &Session,
        config: ClientConfig,
    ) -> Result<(Self, XivaEvents), ProtoError> {
        let registry = RegistryClient::new(&config.registry, &config.http, &session.cookies)?;
        let media = MediaClient::new(config.media, &config.http, session.cookies.clone())?;
        let user = session.identity.xiva_user().to_owned();
        let (xiva, events) = XivaHandle::spawn(connector, config.xiva, user, session.cookies.clone());
        let client = Self {
            xiva,
            registry,
            media,
            excluded_namespaces: config.excluded_namespaces,
            history_limit: config.history_limit,
        };
        Ok((client, events))
    }

    /// All chats with their newest message, in server order.
    pub async fn chat_list(&self) -> Result<Vec<WireChat>, ProtoError> {
        self.xiva.chat_list(&self.excluded_namespaces).await
    }

    /// One page of up to `history_limit` messages older than `before` (newest page when `None`).
    pub async fn history(&self, chat: &str, before: Option<i64>) -> Result<WireHistory, ProtoError> {
        self.xiva.history(chat, before, self.history_limit).await
    }

    /// Sends one write. Fails fast with `ProtoError::Network` while the push socket is not
    /// subscribed; a refusing commit status is `ProtoError::Commit`.
    pub async fn push(&self, request: WriteRequest) -> Result<Committed, ProtoError> {
        self.xiva.push(request).await
    }

    pub async fn users(&self, guids: &[String]) -> Result<Vec<WireUser>, ProtoError> {
        self.registry.users(guids).await
    }

    /// The app's search: first page of messages, users and chats matching `query`.
    pub async fn search(&self, query: &str) -> Result<SearchReply, ProtoError> {
        self.search_with(query, SEARCH_LIMIT, &[SearchEntity::Messages, SearchEntity::Users, SearchEntity::Chats]).await
    }

    /// `search` with an explicit page size and entity set (what `bt-probe search` exposes).
    pub async fn search_with(&self, query: &str, limit: u32, entities: &[SearchEntity]) -> Result<SearchReply, ProtoError> {
        self.registry.search(query, limit, entities).await
    }

    pub async fn fetch_media(&self, target: &MediaTarget) -> Result<Vec<u8>, ProtoError> {
        self.media.fetch(target).await
    }

    /// Streams an original file into `out` without buffering it and without a size cap or
    /// total deadline (see [`MediaClient::download`]). Returns the byte count.
    pub async fn download_media(
        &self,
        target: &MediaTarget,
        out: &mut (dyn Write + Send),
        progress: &(dyn Fn(u64, Option<u64>) + Send + Sync),
    ) -> Result<u64, ProtoError> {
        self.media.download(target, out, progress).await
    }

    /// Chats to subscribe for live events (see the live-update assumptions in the design).
    pub fn watch_chats(&self, chats: Vec<String>) {
        self.xiva.watch_chats(chats);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Identity;

    #[tokio::test]
    async fn start_builds_a_client_offline() {
        let session = Session { cookies: vec![], identity: Identity { uid: None, guid: "g".into(), display_name: None } };
        let mut config = ClientConfig::production();
        // Nothing listens here: connection attempts fail and requests fail fast.
        config.xiva.base_url = "ws://127.0.0.1:1/v2".into();
        let (client, _events) = Client::start(&session, config).unwrap();
        tokio::task::yield_now().await;
        assert!(client.chat_list().await.is_err());
        assert_eq!(client.history_limit, 50);
    }
}
