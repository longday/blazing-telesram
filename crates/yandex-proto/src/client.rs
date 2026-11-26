//! Facade over the socket, registry and media clients: reads, and the closed write set of
//! [`WriteRequest`]. This is what the adapter in `bt-chat-core` maps to its `Backend` trait.

use std::io::Write;

use crate::bootstrap::RegistryEndpoints;
use crate::error::ProtoError;
use crate::manage::{CreatedChat, ManageError, ManageRequest, created_chat_id, parse_created_chat};
use crate::media::{MediaClient, MediaConfig, MediaTarget};
use crate::net::HttpSettings;
use crate::registry::RegistryClient;
use crate::request::{HISTORY_CHUNK, ReadRequest, SEARCH_LIMIT, SearchEntity};
use crate::search::SearchReply;
use crate::session::Session;
use crate::wire::{
    DEFAULT_EXCLUDED_NAMESPACES, WireChat, WireHistory, WireMembers, WireProfile, WireReactors,
    WireUser, parse_members, parse_profile, parse_reactors,
};
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
    guid: String,
}

impl Client {
    /// Connects to production Xiva. Must run inside a Tokio runtime; the returned stream
    /// carries connection state and pushes.
    pub fn start(
        session: &Session,
        config: ClientConfig,
    ) -> Result<(Self, XivaEvents), ProtoError> {
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
        let (xiva, events) =
            XivaHandle::spawn(connector, config.xiva, user, session.cookies.clone());
        let client = Self {
            xiva,
            registry,
            media,
            excluded_namespaces: config.excluded_namespaces,
            history_limit: config.history_limit,
            guid: session.identity.guid.clone(),
        };
        Ok((client, events))
    }

    /// All chats with their newest message, in server order.
    pub async fn chat_list(&self) -> Result<Vec<WireChat>, ProtoError> {
        self.xiva.chat_list(&self.excluded_namespaces).await
    }

    /// One page of up to `history_limit` messages older than `before` (newest page when `None`).
    pub async fn history(
        &self,
        chat: &str,
        before: Option<i64>,
    ) -> Result<WireHistory, ProtoError> {
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
        self.search_with(
            query,
            SEARCH_LIMIT,
            &[
                SearchEntity::Messages,
                SearchEntity::Users,
                SearchEntity::Chats,
            ],
        )
        .await
    }

    /// `search` with an explicit page size and entity set (what `bt-probe search` exposes).
    pub async fn search_with(
        &self,
        query: &str,
        limit: u32,
        entities: &[SearchEntity],
    ) -> Result<SearchReply, ProtoError> {
        self.registry.search(query, limit, entities).await
    }

    /// The signed-in account's profile (socket `whoami`).
    pub async fn whoami(&self) -> Result<WireProfile, ProtoError> {
        parse_profile(
            &self
                .xiva
                .request(ReadRequest::Whoami {
                    guid: self.guid.clone(),
                })
                .await?,
        )
    }

    /// One page of up to `limit` members of a **group** chat (`get_chat_members`): the first page
    /// when `guid_offset` is `None`, else the users after that guid. The server sends no paging
    /// marker, so a page shorter than `limit` is the last. Private chats, the self chat and
    /// channels are refused with `no_permissions_for_this_chat_operation`, which surfaces as
    /// `ProtoError::Protocol` carrying that code.
    pub async fn members(
        &self,
        chat: &str,
        limit: u32,
        guid_offset: Option<&str>,
    ) -> Result<WireMembers, ProtoError> {
        let request = ReadRequest::ChatMembers {
            chat: chat.to_owned(),
            limit,
            guid_offset: guid_offset.map(str::to_owned),
            roles: None,
        };
        parse_members(&self.registry.execute(&request).await?)
    }

    /// One page of up to `limit` users who reacted to the message `timestamp` of `chat`
    /// (socket `list_reactions`, `Mode` 0), in server order. `before` is the cursor for an older
    /// page (sent as `MaxTimestamp`); `None` asks for the first page. The reply carries no total
    /// and no paging marker, see [`WireReactors`]. A refused read (deleted message, unreadable
    /// chat) is the usual `ProtoError`.
    pub async fn reactors(
        &self,
        chat: &str,
        timestamp: i64,
        before: Option<i64>,
        limit: u32,
    ) -> Result<WireReactors, ProtoError> {
        let request = ReadRequest::ListReactions {
            chat: chat.to_owned(),
            timestamp,
            before,
            limit,
        };
        parse_reactors(&self.xiva.request(request).await?)
    }

    /// Opens (creating it when missing) the private chat with the user `guid`: one CSRF-guarded
    /// registry mutation, sent at most once and never retried. `Ok` is the server's `ok` answer;
    /// its value is the id of the chat when the reply names one and `None` otherwise (a success
    /// without an id is still a success; the caller refreshes the chat list). A failure says
    /// whether a repeat could duplicate the chat, see [`ManageError`].
    pub async fn create_private_chat(&self, guid: &str) -> Result<Option<String>, ManageError> {
        let data = self
            .registry
            .manage(&ManageRequest::CreatePrivateChat {
                guid: guid.to_owned(),
            })
            .await?;
        Ok(created_chat_id(&data))
    }

    /// Creates a group chat named `title` with the participants `users` (guids, never the
    /// caller's own): one CSRF-guarded registry mutation, sent at most once and never retried.
    /// `Ok` is the server's `ok` answer; `CreatedChat::chat` is the id of the group when the
    /// reply names one and `refused` lists the codes for the participants the server could not
    /// add (the group exists anyway). A failure says whether a repeat could duplicate the group,
    /// see [`ManageError`]. The payload is the web client's, not yet confirmed live.
    pub async fn create_chat(
        &self,
        title: &str,
        users: &[String],
    ) -> Result<CreatedChat, ManageError> {
        let request = ManageRequest::CreateChat {
            title: title.to_owned(),
            users: users.to_vec(),
        };
        Ok(parse_created_chat(&self.registry.manage(&request).await?))
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
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use serde_json::{Value as Json, json};
    use tokio::sync::mpsc;

    use crate::frame::{IncomingFrame, RequestFrame};
    use crate::xiva::{ConnectError, Socket, SocketEvent, Target, XivaEvent};

    #[tokio::test]
    async fn start_builds_a_client_offline() {
        let session = Session {
            cookies: vec![],
            identity: Identity {
                uid: None,
                guid: "g".into(),
                display_name: None,
            },
        };
        let mut config = ClientConfig::production();
        // Nothing listens here: connection attempts fail and requests fail fast.
        config.xiva.base_url = "ws://127.0.0.1:1/v2".into();
        let (client, _events) = Client::start(&session, config).unwrap();
        tokio::task::yield_now().await;
        assert!(client.chat_list().await.is_err());
        assert_eq!(client.history_limit, 50);
        assert_eq!(client.guid, "g");
        assert!(client.whoami().await.is_err());
        assert!(client.reactors("0/0/c", 1, None, 30).await.is_err());
    }

    async fn client_with_registry(base: &str) -> Client {
        let session = Session {
            cookies: vec![],
            identity: Identity {
                uid: None,
                guid: "g".into(),
                display_name: None,
            },
        };
        let mut config = ClientConfig::production();
        config.xiva.base_url = "ws://127.0.0.1:1/v2".into();
        config.registry.api_url = format!("{base}/api/");
        config.registry.csrf_url = format!("{base}/csrf/");
        Client::start(&session, config).unwrap().0
    }

    #[tokio::test]
    async fn members_sends_the_paged_body_and_parses_the_live_group_page() {
        use crate::registry::stub::{json_response, serve};
        use crate::wire::WireMemberRole;
        let page = include_str!("../tests/fixtures/live-members-group.json");
        let reply = format!(r#"{{"status":"ok","data":{page}}}"#);
        let (base, seen) = serve(vec![
            json_response("200 OK", &reply),
            json_response("200 OK", &reply),
        ]);
        let client = client_with_registry(&base).await;

        let members = client.members("0/0/c", 50, Some("g-last")).await.unwrap();
        assert_eq!(members.users.len(), 4);
        assert_eq!(members.users[0].role, WireMemberRole::Admin);
        let request = seen.lock().unwrap()[0].clone();
        assert!(
            request.contains("get_chat_members")
                && !request.to_lowercase().contains("x-csrf-token")
        );
        assert!(request.contains(r#""chat_id":"0/0/c""#) && request.contains(r#""limit":50"#));
        assert!(request.contains(r#""guid_offset":"g-last""#) && !request.contains("roles"));

        client.members("0/0/c", 50, None).await.unwrap();
        assert!(!seen.lock().unwrap()[1].contains("guid_offset"));
    }

    #[tokio::test]
    async fn a_refused_member_read_surfaces_the_server_code() {
        use crate::registry::stub::{json_response, serve};
        let refusal = include_str!("../tests/fixtures/live-members-private.json");
        let (base, _) = serve(vec![json_response("403 Forbidden", refusal)]);
        let err = client_with_registry(&base)
            .await
            .members("a_b", 50, None)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ProtoError::Protocol(m) if m.contains("no_permissions_for_this_chat_operation")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn create_private_chat_sends_one_guarded_request_and_returns_the_chat_id() {
        use crate::registry::stub::{json_response, serve};
        let (base, seen) = serve(vec![
            json_response("200 OK", r#"{"token":"t0k"}"#),
            json_response("200 OK", r#"{"status":"ok","data":{"chat_id":"a_b"}}"#),
        ]);
        let created = client_with_registry(&base)
            .await
            .create_private_chat("g-target")
            .await
            .unwrap();
        assert_eq!(created.as_deref(), Some("a_b"));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "one CSRF exchange and one creation");
        let request = seen[1].to_lowercase();
        assert!(request.contains("x-csrf-token: t0k"), "{request}");
        assert!(
            request.contains("create_private_chat") && request.contains("g-target"),
            "{request}"
        );
    }

    #[tokio::test]
    async fn create_private_chat_success_without_an_id_is_none_and_failures_keep_their_class() {
        use crate::registry::stub::{json_response, serve};
        let csrf = || json_response("200 OK", r#"{"token":"t0k"}"#);
        let (base, _) = serve(vec![
            csrf(),
            json_response("200 OK", r#"{"status":"ok"}"#),
            csrf(),
            json_response(
                "200 OK",
                r#"{"status":"error","data":{"code":"no_permission"}}"#,
            ),
            csrf(),
            json_response("503 Service Unavailable", "{}"),
        ]);
        let client = client_with_registry(&base).await;
        assert_eq!(client.create_private_chat("g").await, Ok(None));
        assert!(matches!(
            client.create_private_chat("g").await,
            Err(ManageError::Rejected(_))
        ));
        assert!(matches!(
            client.create_private_chat("g").await,
            Err(ManageError::Unknown(_))
        ));
    }

    #[tokio::test]
    async fn create_chat_sends_one_guarded_request_with_the_recorded_payload() {
        use crate::registry::stub::{json_response, serve};
        let (base, seen) = serve(vec![
            json_response("200 OK", r#"{"token":"t0k"}"#),
            json_response(
                "200 OK",
                r#"{"status":"ok","data":{"chat":{"chat_id":"0/0/g"},"errors":[{"code":"join_denied"}]}}"#,
            ),
        ]);
        let users = ["g-1".to_owned(), "g-2".to_owned()];
        let created = client_with_registry(&base)
            .await
            .create_chat("Test group", &users)
            .await
            .unwrap();
        assert_eq!(created.chat.as_deref(), Some("0/0/g"));
        assert_eq!(created.refused, ["join_denied"]);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "one CSRF exchange and one creation");
        let request = seen[1].to_lowercase();
        assert!(request.contains("x-csrf-token: t0k"), "{request}");
        assert!(
            request.contains("create_chat") && request.contains("test group"),
            "{request}"
        );
        assert!(
            request.contains(r#""users":["g-1","g-2"]"#) && request.contains(r#""is_public":true"#),
            "{request}"
        );
    }

    #[tokio::test]
    async fn create_chat_success_without_an_id_and_failures_keep_their_class() {
        use crate::registry::stub::{json_response, serve};
        let csrf = || json_response("200 OK", r#"{"token":"t0k"}"#);
        let (base, _) = serve(vec![
            csrf(),
            json_response("200 OK", r#"{"status":"ok"}"#),
            csrf(),
            json_response(
                "200 OK",
                r#"{"status":"error","data":{"code":"validation_error"}}"#,
            ),
            csrf(),
            json_response("503 Service Unavailable", "{}"),
        ]);
        let client = client_with_registry(&base).await;
        let users = ["g".to_owned()];
        assert_eq!(
            client.create_chat("T", &users).await,
            Ok(CreatedChat::default())
        );
        assert!(matches!(
            client.create_chat("T", &users).await,
            Err(ManageError::Rejected(_))
        ));
        assert!(matches!(
            client.create_chat("T", &users).await,
            Err(ManageError::Unknown(_))
        ));
    }

    /// A socket that answers each request with the next scripted body (echoing its `RequestId`)
    /// and records the method and body it was asked for.
    struct ScriptedSocket {
        incoming: mpsc::UnboundedReceiver<SocketEvent>,
        echo: mpsc::UnboundedSender<SocketEvent>,
        sent: Arc<Mutex<Vec<(String, Json)>>>,
        replies: VecDeque<Json>,
    }

    impl Socket for ScriptedSocket {
        async fn send(&mut self, frame: Vec<u8>) -> Result<(), String> {
            let IncomingFrame::Reply {
                req_id,
                method,
                body,
                ..
            } = IncomingFrame::decode(&frame).map_err(|e| e.to_string())?
            else {
                return Err("not a request frame".into());
            };
            let mut reply = self.replies.pop_front().ok_or("no reply scripted")?;
            reply["RequestId"] = body["RequestId"].clone();
            self.sent.lock().unwrap().push((method.clone(), body));
            let bytes = RequestFrame {
                service_index: 0,
                req_id,
                method,
                body: reply,
            }
            .encode();
            self.echo
                .send(SocketEvent::Binary(bytes))
                .map_err(|e| e.to_string())
        }
        async fn recv(&mut self) -> SocketEvent {
            self.incoming.recv().await.unwrap_or(SocketEvent::Closed {
                code: None,
                reason: "eof".into(),
            })
        }
        async fn close(&mut self) {}
    }

    struct ScriptedConnector(Mutex<Option<ScriptedSocket>>);

    impl Connector for ScriptedConnector {
        type Socket = ScriptedSocket;
        async fn connect(&self, _: &Target) -> Result<ScriptedSocket, ConnectError> {
            self.0
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| ConnectError::Failed("one connection only".into()))
        }
    }

    /// A connected client whose socket answers with `replies` in order.
    async fn client_with_replies(replies: Vec<Json>) -> (Client, Arc<Mutex<Vec<(String, Json)>>>) {
        let (echo, incoming) = mpsc::unbounded_channel();
        echo.send(SocketEvent::Text(
            json!({ "operation": "subscribed", "subscription-id": "s1" }).to_string(),
        ))
        .unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let socket = ScriptedSocket {
            incoming,
            echo,
            sent: sent.clone(),
            replies: replies.into(),
        };
        let session = Session {
            cookies: vec![],
            identity: Identity {
                uid: None,
                guid: "g".into(),
                display_name: None,
            },
        };
        let (client, mut events) = Client::start_with(
            ScriptedConnector(Mutex::new(Some(socket))),
            &session,
            ClientConfig::production(),
        )
        .unwrap();
        while !matches!(events.recv().await, Some(XivaEvent::Connected { .. })) {}
        (client, sent)
    }

    #[tokio::test(start_paused = true)]
    async fn reactors_sends_the_list_reactions_request_and_parses_the_page() {
        let page: Json = serde_json::from_str(include_str!(
            "../tests/fixtures/research-reactors-page.json"
        ))
        .unwrap();
        let (client, sent) = client_with_replies(vec![page.clone(), page]).await;

        let first = client.reactors("0/0/c", 111, None, 30).await.unwrap();
        assert_eq!(first.entries.len(), 4);
        assert_eq!((first.raw, first.skipped), (4, 0));
        assert_eq!(first.oldest, Some(1795171900000000));
        assert_eq!(
            first.entries[0].guid,
            "00000007-0000-4000-8000-000000000001"
        );

        let older = client.reactors("0/0/c", 111, Some(99), 15).await.unwrap();
        assert_eq!(older, first);

        let sent = sent.lock().unwrap();
        let (method, body) = &sent[0];
        assert_eq!(method, "list_reactions");
        assert_eq!(body["ChatId"], "0/0/c");
        assert_eq!(body["Timestamp"], 111);
        assert_eq!(body["Limit"], 30);
        assert_eq!(body["Mode"], 0);
        assert!(body.get("MaxTimestamp").is_none(), "{body}");
        let (method, body) = &sent[1];
        assert_eq!(method, "list_reactions");
        assert_eq!(
            (body["Limit"].as_u64(), body["MaxTimestamp"].as_i64()),
            (Some(15), Some(99))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_refused_reactors_read_is_the_usual_proto_error() {
        let (client, _) = client_with_replies(vec![
            json!({ "Status": 4, "Details": "no such message" }),
            json!({ "Status": 5 }),
            json!({ "UserReactions": 7 }),
            json!({ "Status": 6 }),
        ])
        .await;
        let err = client.reactors("0/0/c", 1, None, 30).await.unwrap_err();
        assert!(
            matches!(&err, ProtoError::Protocol(m) if m.contains("ENTITY_NOT_FOUND") && m.contains("no such message")),
            "{err:?}"
        );
        assert!(matches!(
            client.reactors("0/0/c", 1, None, 30).await,
            Err(ProtoError::Network(_))
        ));
        assert!(matches!(
            client.reactors("0/0/c", 1, None, 30).await,
            Err(ProtoError::Protocol(m)) if m.contains("not an array")
        ));
        assert_eq!(
            client.reactors("0/0/c", 1, None, 30).await,
            Err(ProtoError::SessionExpired)
        );
    }
}
