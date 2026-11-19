//! Xiva push WebSocket: connection actor with reconnect/backoff, request/reply over the
//! socket (`history`, `subscribe`), operation frames (`subscribed`, `ping`) and push decoding.
//! The socket is abstracted by [`Connector`]/[`Socket`] so tests run on a fake.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value as Json;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tracing::{debug, info, trace, warn};

use crate::error::ProtoError;
use crate::frame::IncomingFrame;
use crate::net::{millis, redact_url};
use crate::request::{ReadRequest, Route, SocketContext};
use crate::session::{SessionCookie, cookie_header};
use crate::wire::{PushEvent, check_status, decode_push};

const TOPICS: &str = "version5*common+version5*main";
/// Chat subscriptions live 60 s; refresh at half that.
const SUBSCRIBE_REFRESH: Duration = Duration::from_secs(30);
/// Close reasons that mean the cookie session is dead (research §14.5).
const AUTH_CLOSE_REASONS: [&str; 3] = ["cookie auth failed", "no credentials", "bad sign"];

#[derive(Clone, Debug)]
pub struct XivaConfig {
    pub base_url: String,
    pub service: String,
    /// First reconnect delay: the web client's `reconnectInterval`.
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    pub request_timeout: Duration,
    /// Deadline for the first server `ping` after open, then 1.3 x its interval.
    pub first_ping_timeout: Duration,
}

impl XivaConfig {
    pub fn production() -> Self {
        Self {
            base_url: "wss://push.yandex.ru/v2/subscribe/websocket".into(),
            service: "messenger-prod".into(),
            backoff_base: Duration::from_secs(11),
            backoff_max: Duration::from_secs(176),
            request_timeout: Duration::from_secs(30),
            first_ping_timeout: Duration::from_secs(15),
        }
    }
}

/// What a connector needs to open one connection. `Debug` hides the cookie.
#[derive(Clone)]
pub struct Target {
    pub url: String,
    pub cookie: String,
    pub origin: String,
}

impl fmt::Debug for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Target").field("url", &self.url).field("cookie", &"<redacted>").finish()
    }
}

#[derive(Debug)]
pub enum ConnectError {
    /// The handshake was refused with 401/403.
    Unauthorized,
    Failed(String),
}

#[derive(Debug)]
pub enum SocketEvent {
    Binary(Vec<u8>),
    Text(String),
    Closed { code: Option<u16>, reason: String },
    Error(String),
}

pub trait Socket: Send + 'static {
    fn send(&mut self, frame: Vec<u8>) -> impl Future<Output = Result<(), String>> + Send;
    /// Must be cancel-safe: it is raced against commands and timers.
    fn recv(&mut self) -> impl Future<Output = SocketEvent> + Send;
    fn close(&mut self) -> impl Future<Output = ()> + Send;
}

pub trait Connector: Send + Sync + 'static {
    type Socket: Socket;
    fn connect(&self, target: &Target) -> impl Future<Output = Result<Self::Socket, ConnectError>> + Send;
}

#[derive(Debug, Clone, PartialEq)]
pub enum XivaEvent {
    Connecting,
    /// The `subscribed` operation frame arrived: pushes will flow.
    Connected { subscription_id: String },
    Disconnected { reason: String, retry_in: Duration },
    /// Cookies were rejected; the actor stops reconnecting.
    SessionExpired,
    Push(PushEvent),
    /// Something undecodable or refused that did not break the connection.
    Warning(String),
}

type Reply = oneshot::Sender<Result<Json, ProtoError>>;

enum Command {
    Request { request: ReadRequest, reply: Reply },
    Watch(Vec<String>),
}

#[derive(Clone)]
pub struct XivaHandle {
    commands: mpsc::UnboundedSender<Command>,
}

pub type XivaEvents = mpsc::UnboundedReceiver<XivaEvent>;

impl XivaHandle {
    /// Spawns the connection actor on the current Tokio runtime. It stops when every handle
    /// is dropped.
    pub fn spawn<C: Connector>(
        connector: C,
        config: XivaConfig,
        user: String,
        cookies: Vec<SessionCookie>,
    ) -> (Self, XivaEvents) {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::unbounded_channel();
        let handle = Self { commands };
        let connector = Arc::new(connector);
        tokio::spawn(Actor { connector, config, user, cookies, rx: command_rx, events: event_tx, watched: Vec::new() }.run());
        (handle, events)
    }

    /// Replaces the set of chats subscribed for live events; re-sent after every reconnect.
    pub fn watch_chats(&self, chats: Vec<String>) {
        let _ = self.commands.send(Command::Watch(chats));
    }

    /// Socket requests only. Fails fast while offline; callers retry after `Connected`.
    pub(crate) async fn request(&self, request: ReadRequest) -> Result<Json, ProtoError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::Request { request, reply })
            .map_err(|_| ProtoError::Network("connection task stopped".into()))?;
        rx.await.map_err(|_| ProtoError::Network("connection task stopped".into()))?
    }

    pub async fn chat_list(&self, excluded: &[u32]) -> Result<Vec<crate::wire::WireChat>, ProtoError> {
        crate::wire::parse_chat_list(&self.request(ReadRequest::ChatList).await?, excluded)
    }

    pub async fn history(
        &self,
        chat: &str,
        before: Option<i64>,
        limit: u32,
    ) -> Result<crate::wire::WireHistory, ProtoError> {
        let request = ReadRequest::History { chat: chat.to_owned(), before, limit };
        crate::wire::parse_history(&self.request(request).await?, chat, limit)
    }
}

/// `service=messenger-prod%3Aversion5*common%2Bversion5*main&session=..&client=web_main&user=..`;
/// a literal `+` in `service` makes the server close with 4400, so the query is form-encoded.
pub fn build_url(config: &XivaConfig, session: &str, user: &str) -> Result<String, ProtoError> {
    let service = format!("{}:{TOPICS}", config.service);
    reqwest::Url::parse_with_params(
        &config.base_url,
        [("service", service.as_str()), ("session", session), ("client", "web_main"), ("user", user)],
    )
    .map(String::from)
    .map_err(|e| ProtoError::Protocol(format!("invalid push url: {e}")))
}

fn session_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    (0..4).map(|i| &hex[i * 4..i * 4 + 4]).collect::<Vec<_>>().join("-")
}

fn request_id() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..24])
}

/// Idle deadline after a ping advertising `interval_secs`. A missing, non-positive,
/// non-finite or absurd interval falls back to the 60 s default instead of panicking.
fn ping_deadline(interval_secs: Option<f64>) -> Duration {
    let secs = interval_secs.filter(|s| s.is_finite() && *s > 0.0 && *s <= 3600.0).unwrap_or(60.0);
    Duration::from_secs_f64(secs * 1.3)
}

fn transport_name(code: u64) -> &'static str {
    match code {
        1 => "PROTOCOL_ERROR",
        2 => "BACKEND_CALL_ERROR",
        3 => "INTERNAL_ERROR",
        4 => "CORRUPTED_DATA_HEADER",
        5 => "BACKEND_NOT_FOUND",
        6 => "SERVICE_UNAVAILABLE",
        7 => "TOO_MANY_REQUESTS",
        8 => "FRAME_TOO_LARGE",
        _ => "unknown",
    }
}

fn transport_error(code: u64) -> ProtoError {
    let name = transport_name(code);
    if matches!(code, 3 | 6 | 7) {
        ProtoError::Network(format!("transport error {code} {name}"))
    } else {
        ProtoError::Protocol(format!("transport error {code} {name}"))
    }
}

/// Kind name and chat id of a push for the log; never the message content.
fn push_kind(event: &PushEvent) -> (&'static str, &str) {
    match event {
        PushEvent::MessageUpserted(message) => ("MessageUpserted", &message.chat),
        PushEvent::MessageDeleted { chat, .. } => ("MessageDeleted", chat),
        PushEvent::ChatChanged { chat } => ("ChatChanged", chat),
        PushEvent::Ignored { .. } => ("Ignored", ""),
    }
}

struct Pending {
    request_id: String,
    reply: Option<Reply>,
    deadline: Instant,
    sent: Instant,
    /// The subscribe round this request belongs to.
    round: Option<u64>,
}

/// Outcome counts of one subscribe round. The server answers every chat separately and
/// refuses some under load, so the round is summed up in one line instead of per chat.
#[derive(Default)]
struct Round {
    chats: usize,
    ok: usize,
    failed: BTreeMap<String, usize>,
}

impl Round {
    fn refused(&self) -> usize {
        self.failed.values().sum()
    }

    fn unanswered(&self) -> usize {
        self.chats.saturating_sub(self.ok + self.refused())
    }

    fn report(&self) {
        let (chats, ok, refused, unanswered) = (self.chats, self.ok, self.refused(), self.unanswered());
        if refused == 0 && unanswered == 0 {
            debug!(chats, ok, "subscribe round");
        } else {
            let errors = self.failed.iter().map(|(name, n)| format!("{name}={n}")).collect::<Vec<_>>().join(", ");
            warn!(chats, ok, refused, unanswered, %errors, "subscribe round");
        }
    }
}

#[derive(Default)]
struct Rounds {
    next: u64,
    open: HashMap<u64, Round>,
}

impl Rounds {
    fn start(&mut self) -> u64 {
        self.next += 1;
        self.open.insert(self.next, Round::default());
        self.next
    }

    fn sent(&mut self, id: u64) {
        if let Some(round) = self.open.get_mut(&id) {
            round.chats += 1;
        }
    }

    /// Counts one answer (or timeout) and reports the round once every chat has one.
    fn record(&mut self, id: u64, outcome: Result<(), &str>) {
        let Some(round) = self.open.get_mut(&id) else { return };
        match outcome {
            Ok(()) => round.ok += 1,
            Err(name) => *round.failed.entry(name.to_owned()).or_default() += 1,
        }
        if round.unanswered() == 0 {
            round.report();
            self.open.remove(&id);
        }
    }

    /// Reports rounds still waiting for answers when the connection ends.
    fn flush(&mut self) {
        for round in self.open.values().filter(|r| r.chats > 0) {
            round.report();
        }
        self.open.clear();
    }
}

enum End {
    Expired,
    Dropped(String),
    Stopped,
}

struct Actor<C: Connector> {
    connector: Arc<C>,
    config: XivaConfig,
    user: String,
    cookies: Vec<SessionCookie>,
    rx: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<XivaEvent>,
    watched: Vec<String>,
}

impl<C: Connector> Actor<C> {
    fn emit(&self, event: XivaEvent) {
        let _ = self.events.send(event);
    }

    /// Reports something undecodable or refused: to the log and to the event consumer.
    fn warning(&self, message: String) {
        warn!("{message}");
        self.emit(XivaEvent::Warning(message));
    }

    fn backoff(&self, failures: u32) -> Duration {
        self.config.backoff_base.saturating_mul(1u32 << failures.min(10)).min(self.config.backoff_max)
    }

    /// Offline handling of a command; `false` when every handle is gone.
    fn offline(&mut self, command: Option<Command>) -> bool {
        match command {
            None => return false,
            Some(Command::Request { reply, .. }) => {
                let _ = reply.send(Err(ProtoError::Network("not connected".into())));
            }
            Some(Command::Watch(chats)) => self.watched = chats,
        }
        true
    }

    async fn run(mut self) {
        let mut failures = 0u32;
        loop {
            self.emit(XivaEvent::Connecting);
            let target = match self.target() {
                Ok(t) => t,
                Err(e) => {
                    self.warning(e.to_string());
                    return;
                }
            };
            info!(url = %redact_url(&target.url), "connecting");
            let connector = Arc::clone(&self.connector);
            let connect = connector.connect(&target);
            tokio::pin!(connect);
            let result = loop {
                tokio::select! {
                    r = &mut connect => break r,
                    command = self.rx.recv() => if !self.offline(command) { return },
                }
            };
            let reason = match result {
                Err(ConnectError::Unauthorized) => return self.expired().await,
                Err(ConnectError::Failed(reason)) => reason,
                Ok(mut socket) => {
                    let (end, established) = self.session(&mut socket).await;
                    socket.close().await;
                    if established {
                        failures = 0;
                    }
                    match end {
                        End::Expired => return self.expired().await,
                        End::Stopped => return,
                        End::Dropped(reason) => reason,
                    }
                }
            };
            let retry_in = self.backoff(failures);
            failures += 1;
            info!(%reason, retry_in_secs = retry_in.as_secs(), "disconnected");
            self.emit(XivaEvent::Disconnected { reason, retry_in });
            let wake = tokio::time::sleep(retry_in);
            tokio::pin!(wake);
            loop {
                tokio::select! {
                    () = &mut wake => break,
                    command = self.rx.recv() => if !self.offline(command) { return },
                }
            }
        }
    }

    fn target(&self) -> Result<Target, ProtoError> {
        let url = build_url(&self.config, &session_id(), &self.user)?;
        let host = crate::net::host_of(&url)?;
        Ok(Target { cookie: cookie_header(&self.cookies, &host), origin: "https://yandex.ru".into(), url })
    }

    /// Stays alive answering `SessionExpired` until the handles are dropped.
    async fn expired(mut self) {
        info!("session expired; not reconnecting");
        self.emit(XivaEvent::SessionExpired);
        while let Some(command) = self.rx.recv().await {
            if let Command::Request { reply, .. } = command {
                let _ = reply.send(Err(ProtoError::SessionExpired));
            }
        }
    }

    /// Returns how the connection ended and whether `subscribed` was seen.
    async fn session<S: Socket>(&mut self, socket: &mut S) -> (End, bool) {
        let mut next_req = 1u64;
        let mut pending: HashMap<u64, Pending> = HashMap::new();
        let mut rounds = Rounds::default();
        let mut subscription: Option<String> = None;
        let mut idle = Instant::now() + self.config.first_ping_timeout;
        let mut refresh: Option<Instant> = None;
        let mut established = false;

        let end = loop {
            let wake = pending
                .values()
                .map(|p| p.deadline)
                .chain([idle])
                .chain(refresh)
                .min()
                .unwrap_or(idle);
            tokio::select! {
                event = socket.recv() => match event {
                    SocketEvent::Text(text) => {
                        trace!(frame = %text, "operation frame");
                        let op: Json = serde_json::from_str(&text).unwrap_or(Json::Null);
                        match op.get("operation").and_then(Json::as_str) {
                            Some("ping") => {
                                idle = Instant::now() + ping_deadline(op.get("server-interval-sec").and_then(Json::as_f64));
                            }
                            Some("subscribed") => match op.get("subscription-id").and_then(Json::as_str) {
                                Some(id) if !id.is_empty() => {
                                    subscription = Some(id.to_owned());
                                    established = true;
                                    info!(subscription_id = %id, "subscribed");
                                    self.emit(XivaEvent::Connected { subscription_id: id.to_owned() });
                                    if let Err(e) = self.subscribe_all(socket, &mut next_req, &mut pending, &mut rounds, subscription.as_deref()).await {
                                        break End::Dropped(e);
                                    }
                                    refresh = (!self.watched.is_empty()).then(|| Instant::now() + SUBSCRIBE_REFRESH);
                                }
                                _ => self.warning("subscribed frame without subscription-id".into()),
                            },
                            // The server revoked the subscription; a bad session shows on reconnect.
                            Some("unsubscribe") => break End::Dropped("server unsubscribed".into()),
                            _ => {}
                        }
                    }
                    SocketEvent::Binary(bytes) => match IncomingFrame::decode(&bytes) {
                        Err(e) => self.warning(format!("undecodable frame: {e}")),
                        Ok(frame) => {
                            trace!(frame = ?frame, "frame");
                            if self.on_frame(frame, &mut pending, &mut rounds) {
                                break End::Expired;
                            }
                        }
                    },
                    SocketEvent::Closed { code, reason } => {
                        let lower = reason.to_lowercase();
                        if AUTH_CLOSE_REASONS.iter().any(|r| lower.contains(r)) {
                            break End::Expired;
                        }
                        let code = code.map(|c| format!("{c} ")).unwrap_or_default();
                        break End::Dropped(format!("closed: {code}{reason}").trim().to_owned());
                    }
                    SocketEvent::Error(e) => break End::Dropped(e),
                },
                command = self.rx.recv() => match command {
                    None => break End::Stopped,
                    Some(Command::Watch(chats)) => {
                        self.watched = chats;
                        if let Err(e) = self.subscribe_all(socket, &mut next_req, &mut pending, &mut rounds, subscription.as_deref()).await {
                            break End::Dropped(e);
                        }
                        refresh = (!self.watched.is_empty() && subscription.is_some()).then(|| Instant::now() + SUBSCRIBE_REFRESH);
                    }
                    Some(Command::Request { request, reply }) => {
                        if !matches!(request.route(), Route::Socket) {
                            let _ = reply.send(Err(ProtoError::Protocol("not a socket request".into())));
                            continue;
                        }
                        let id = request_id();
                        let ctx = SocketContext { req_id: next_req, request_id: &id, subscription_id: subscription.as_deref() };
                        let Some(frame) = request.socket_frame(&ctx) else {
                            let _ = reply.send(Err(ProtoError::Network("not subscribed yet".into())));
                            continue;
                        };
                        if let Err(e) = socket.send(frame.encode()).await {
                            let _ = reply.send(Err(ProtoError::Network(e.clone())));
                            break End::Dropped(e);
                        }
                        debug!(method = request.method(), req_id = next_req, "socket request");
                        pending.insert(next_req, Pending {
                            request_id: id,
                            reply: Some(reply),
                            deadline: Instant::now() + self.config.request_timeout,
                            sent: Instant::now(),
                            round: None,
                        });
                        next_req += 1;
                    }
                },
                () = tokio::time::sleep_until(wake) => {
                    let now = Instant::now();
                    if now >= idle {
                        break End::Dropped("no server ping in time".into());
                    }
                    let expired: Vec<u64> = pending.iter().filter(|(_, p)| now >= p.deadline).map(|(k, _)| *k).collect();
                    for key in expired {
                        let Some(p) = pending.remove(&key) else { continue };
                        let elapsed_ms = millis(p.sent.elapsed());
                        match (p.reply, p.round) {
                            (Some(reply), _) => {
                                debug!(req_id = key, elapsed_ms, "socket request timed out");
                                let _ = reply.send(Err(ProtoError::Network("request timed out".into())));
                            }
                            (None, Some(round)) => rounds.record(round, Err("timed out")),
                            (None, None) => {}
                        }
                    }
                    if refresh.is_some_and(|t| now >= t) {
                        if let Err(e) = self.subscribe_all(socket, &mut next_req, &mut pending, &mut rounds, subscription.as_deref()).await {
                            break End::Dropped(e);
                        }
                        refresh = Some(now + SUBSCRIBE_REFRESH);
                    }
                }
            }
        };
        rounds.flush();

        let failure = match &end {
            End::Dropped(reason) => ProtoError::Network(format!("connection lost: {reason}")),
            End::Expired => ProtoError::SessionExpired,
            End::Stopped => ProtoError::Network("stopped".into()),
        };
        for (_, p) in pending {
            if let Some(reply) = p.reply {
                let _ = reply.send(Err(failure.clone()));
            }
        }
        (end, established)
    }

    /// Returns true when a reply says the session is rejected: nothing on this socket can work then.
    fn on_frame(&self, frame: IncomingFrame, pending: &mut HashMap<u64, Pending>, rounds: &mut Rounds) -> bool {
        match frame {
            IncomingFrame::Reply { req_id, method, body, .. } => {
                let Some(p) = pending.remove(&req_id) else {
                    self.warning(format!("reply {req_id} without a request"));
                    return false;
                };
                let mismatch = body.get("RequestId").and_then(Json::as_str).is_some_and(|r| r != p.request_id);
                let result = if mismatch {
                    Err(ProtoError::Protocol("RequestId of the reply does not match".into()))
                } else {
                    check_status(&body).map(|()| body)
                };
                let rejected = matches!(result, Err(ProtoError::SessionExpired));
                let elapsed_ms = millis(p.sent.elapsed());
                let error = result.as_ref().err().map(tracing::field::display);
                // Subscribe replies are per chat; their round is logged once.
                if p.reply.is_some() {
                    debug!(req_id, method = %method, ok = error.is_none(), elapsed_ms, error, "socket reply");
                } else {
                    trace!(req_id, method = %method, ok = error.is_none(), elapsed_ms, error, "socket reply");
                }
                if let Some(round) = p.round {
                    let failure = result.as_ref().err().map(ToString::to_string);
                    rounds.record(round, failure.as_deref().map_or(Ok(()), Err));
                }
                match p.reply {
                    Some(reply) => {
                        let _ = reply.send(result);
                    }
                    // A rejected session is reported as `SessionExpired` once the socket ends.
                    None if rejected => {}
                    None => {
                        if let Err(e) = result {
                            self.warning(format!("subscribe refused: {e}"));
                        }
                    }
                }
                rejected
            }
            IncomingFrame::ProxyStatus { req_id, error_code } => {
                if let Some(p) = pending.remove(&req_id) {
                    let elapsed_ms = millis(p.sent.elapsed());
                    debug!(req_id, error_code, elapsed_ms, "socket transport error");
                    if let Some(round) = p.round {
                        rounds.record(round, Err(transport_name(error_code)));
                    }
                    if let Some(reply) = p.reply {
                        let _ = reply.send(Err(transport_error(error_code)));
                    }
                }
                false
            }
            IncomingFrame::Push { body, .. } => {
                match decode_push(&body) {
                    Ok(PushEvent::Ignored { kind }) => debug!(%kind, "push ignored"),
                    Ok(event) => {
                        let (kind, chat) = push_kind(&event);
                        debug!(kind, chat, "push");
                        self.emit(XivaEvent::Push(event));
                    }
                    Err(e) => self.warning(format!("undecodable push: {e}")),
                }
                false
            }
        }
    }

    /// Sends `subscribe` for every watched chat; the answers are summed up per round.
    async fn subscribe_all<S: Socket>(
        &self,
        socket: &mut S,
        next_req: &mut u64,
        pending: &mut HashMap<u64, Pending>,
        rounds: &mut Rounds,
        subscription: Option<&str>,
    ) -> Result<(), String> {
        if subscription.is_none() || self.watched.is_empty() {
            return Ok(());
        }
        let round = rounds.start();
        for chat in &self.watched {
            let id = request_id();
            let ctx = SocketContext { req_id: *next_req, request_id: &id, subscription_id: subscription };
            let Some(frame) = (ReadRequest::SubscribeChat { chat: chat.clone() }).socket_frame(&ctx) else {
                continue;
            };
            socket.send(frame.encode()).await?;
            rounds.sent(round);
            let now = Instant::now();
            pending.insert(
                *next_req,
                Pending { request_id: id, reply: None, deadline: now + self.config.request_timeout, sent: now, round: Some(round) },
            );
            *next_req += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use rmpv::Value;
    use serde_json::json;
    use tokio::task::yield_now;

    use crate::test_log::Capture;

    use super::*;

    struct FakeSocket {
        incoming: mpsc::UnboundedReceiver<SocketEvent>,
        sent: mpsc::UnboundedSender<Vec<u8>>,
    }

    impl Socket for FakeSocket {
        async fn send(&mut self, frame: Vec<u8>) -> Result<(), String> {
            self.sent.send(frame).map_err(|e| e.to_string())
        }
        async fn recv(&mut self) -> SocketEvent {
            self.incoming.recv().await.unwrap_or(SocketEvent::Closed { code: None, reason: "eof".into() })
        }
        async fn close(&mut self) {}
    }

    /// Server side of one fake connection.
    struct Server {
        push: mpsc::UnboundedSender<SocketEvent>,
        sent: mpsc::UnboundedReceiver<Vec<u8>>,
    }

    impl Server {
        fn text(&self, v: Json) {
            self.push.send(SocketEvent::Text(v.to_string())).unwrap();
        }
        fn subscribed(&self, id: &str) {
            self.text(json!({ "operation": "subscribed", "subscription-id": id }));
        }
        fn close(&self, reason: &str) {
            self.push.send(SocketEvent::Closed { code: Some(1000), reason: reason.into() }).unwrap();
        }
        async fn next_request(&mut self) -> (u64, String, Json) {
            let bytes = self.sent.recv().await.unwrap();
            match IncomingFrame::decode(&bytes).unwrap() {
                IncomingFrame::Reply { req_id, method, body, .. } => (req_id, method, body),
                other => panic!("{other:?}"),
            }
        }
        fn reply(&self, req_id: u64, method: &str, mut body: Json) {
            let header = rmpv::Value::Array(vec![Value::from(0u64), Value::from(req_id), Value::from(method)]);
            let mut frame = vec![0x01];
            rmpv::encode::write_value(&mut frame, &header).unwrap();
            frame.push(0x05);
            frame.extend_from_slice(&[0; 11]);
            if body.is_null() {
                body = json!({});
            }
            frame.extend_from_slice(body.to_string().as_bytes());
            self.push.send(SocketEvent::Binary(frame)).unwrap();
        }
        fn push_frame(&self, body: Json) {
            let header = Value::Array(vec![Value::from(1u64), Value::from("messenger"), Value::from("event"), Value::from(1u64)]);
            let mut frame = vec![0x03];
            rmpv::encode::write_value(&mut frame, &header).unwrap();
            frame.push(0x05);
            frame.extend_from_slice(&[0; 11]);
            frame.extend_from_slice(body.to_string().as_bytes());
            self.push.send(SocketEvent::Binary(frame)).unwrap();
        }
        fn proxy_status(&self, req_id: u64, error_code: u64) {
            let header = Value::Array(vec![Value::from(req_id), Value::from(error_code)]);
            let mut frame = vec![0x02];
            rmpv::encode::write_value(&mut frame, &header).unwrap();
            self.push.send(SocketEvent::Binary(frame)).unwrap();
        }
    }

    #[derive(Clone, Default)]
    struct FakeConnector {
        script: Arc<Mutex<VecDeque<Result<FakeSocket, ConnectError>>>>,
        targets: Arc<Mutex<Vec<Target>>>,
    }

    impl FakeConnector {
        fn accept(&self) -> Server {
            let (push, incoming) = mpsc::unbounded_channel();
            let (sent_tx, sent) = mpsc::unbounded_channel();
            self.script.lock().unwrap().push_back(Ok(FakeSocket { incoming, sent: sent_tx }));
            Server { push, sent }
        }
        fn refuse(&self, e: ConnectError) {
            self.script.lock().unwrap().push_back(Err(e));
        }
    }

    impl Connector for FakeConnector {
        type Socket = FakeSocket;
        async fn connect(&self, target: &Target) -> Result<FakeSocket, ConnectError> {
            self.targets.lock().unwrap().push(target.clone());
            self.script.lock().unwrap().pop_front().unwrap_or(Err(ConnectError::Failed("script exhausted".into())))
        }
    }

    fn cookie() -> SessionCookie {
        SessionCookie {
            name: "Session_id".into(),
            value: "secret".into(),
            domain: ".yandex.ru".into(),
            path: "/".into(),
            expires_unix: None,
            secure: true,
            http_only: true,
        }
    }

    fn start(connector: &FakeConnector) -> (XivaHandle, XivaEvents) {
        XivaHandle::spawn(connector.clone(), XivaConfig::production(), "4001".into(), vec![cookie()])
    }

    async fn next(events: &mut XivaEvents) -> XivaEvent {
        events.recv().await.expect("event")
    }

    #[test]
    fn url_encodes_the_service_like_the_web_client() {
        let url = build_url(&XivaConfig::production(), "aaaa-bbbb-cccc-dddd", "4001").unwrap();
        assert_eq!(
            url,
            "wss://push.yandex.ru/v2/subscribe/websocket?service=messenger-prod%3Aversion5*common%2Bversion5*main&session=aaaa-bbbb-cccc-dddd&client=web_main&user=4001"
        );
        let id = session_id();
        assert_eq!(id.len(), 19);
        assert_eq!(id.matches('-').count(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn connects_with_cookie_and_reports_subscription() {
        let connector = FakeConnector::default();
        let server = connector.accept();
        let (_handle, mut events) = start(&connector);
        assert_eq!(next(&mut events).await, XivaEvent::Connecting);
        server.subscribed("abc123");
        assert_eq!(next(&mut events).await, XivaEvent::Connected { subscription_id: "abc123".into() });
        let target = connector.targets.lock().unwrap()[0].clone();
        assert_eq!(target.cookie, "Session_id=secret");
        assert!(target.url.contains("user=4001"));
        assert!(!format!("{target:?}").contains("secret"));
    }

    #[tokio::test(start_paused = true)]
    async fn requests_are_correlated_by_req_id_and_request_id() {
        let connector = FakeConnector::default();
        let mut server = connector.accept();
        let (handle, mut events) = start(&connector);
        next(&mut events).await;
        let h = handle.clone();
        let task = tokio::spawn(async move { h.history("c", Some(99), 50).await });
        let (req_id, method, body) = server.next_request().await;
        assert_eq!((req_id, method.as_str()), (1, "history"));
        assert_eq!((body["ChatId"].as_str(), body["MaxTimestamp"].as_i64(), body["Limit"].as_u64()), (Some("c"), Some(99), Some(50)));
        let reply = json!({ "RequestId": body["RequestId"], "Chats": [{
            "ChatId": "c",
            "Messages": [{ "ServerMessage": { "ClientMessage": { "Plain": { "ChatId": "c", "Text": { "MessageText": "x" } } }, "ServerMessageInfo": { "Timestamp": 5 } } }]
        }] });
        server.reply(req_id, "history", reply);
        let page = task.await.unwrap().unwrap();
        assert_eq!(page.messages.len(), 1);
        assert!(page.reached_start);

        // A reply carrying a foreign RequestId is rejected.
        let h = handle.clone();
        let task = tokio::spawn(async move { h.chat_list(&[]).await });
        let (req_id, _, _) = server.next_request().await;
        assert_eq!(req_id, 2);
        server.reply(req_id, "history", json!({ "RequestId": "other" }));
        assert!(matches!(task.await.unwrap(), Err(ProtoError::Protocol(_))));
    }

    #[tokio::test(start_paused = true)]
    async fn rejected_reply_fails_the_request_and_expires_the_session() {
        let connector = FakeConnector::default();
        let mut server = connector.accept();
        let (handle, mut events) = start(&connector);
        next(&mut events).await;
        let h = handle.clone();
        let task = tokio::spawn(async move { h.chat_list(&[]).await });
        let (req_id, _, _) = server.next_request().await;
        server.reply(req_id, "history", json!({ "Status": 6 }));
        assert_eq!(task.await.unwrap(), Err(ProtoError::SessionExpired));
        assert_eq!(next(&mut events).await, XivaEvent::SessionExpired);
        assert_eq!(connector.targets.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn unanswered_request_times_out() {
        let connector = FakeConnector::default();
        let mut server = connector.accept();
        let (handle, mut events) = start(&connector);
        next(&mut events).await;
        // No reply at all: the request times out while pings keep the connection alive.
        let h = handle.clone();
        let task = tokio::spawn(async move { h.chat_list(&[]).await });
        server.next_request().await;
        server.text(json!({ "operation": "ping", "server-interval-sec": 60 }));
        assert!(matches!(task.await.unwrap(), Err(ProtoError::Network(m)) if m.contains("timed out")));
    }

    #[tokio::test(start_paused = true)]
    async fn pushes_become_events_and_ignored_ones_are_dropped() {
        let connector = FakeConnector::default();
        let server = connector.accept();
        let (_handle, mut events) = start(&connector);
        next(&mut events).await;
        server.push_frame(json!({ "ClientMessage": { "Typing": { "ChatId": "c" } } }));
        server.push_frame(json!({ "ClientMessage": { "Plain": { "ChatId": "c" } }, "ServerMessageInfo": { "Timestamp": 5, "Deleted": true } }));
        server.push_frame(json!({ "nonsense": true }));
        assert_eq!(next(&mut events).await, XivaEvent::Push(PushEvent::MessageDeleted { chat: "c".into(), timestamp: 5 }));
        assert!(matches!(next(&mut events).await, XivaEvent::Warning(_)));
    }

    #[tokio::test(start_paused = true)]
    async fn reconnects_with_backoff_from_11s_and_resets_after_subscribed() {
        let connector = FakeConnector::default();
        let first = connector.accept();
        let (_handle, mut events) = start(&connector);
        next(&mut events).await;
        first.close("going away");
        let XivaEvent::Disconnected { retry_in, reason } = next(&mut events).await else { panic!() };
        assert_eq!(retry_in, Duration::from_secs(11));
        assert!(reason.contains("going away"));

        // The second connection never gets `subscribed`: the delay doubles.
        let second = connector.accept();
        assert_eq!(next(&mut events).await, XivaEvent::Connecting);
        second.close("again");
        let XivaEvent::Disconnected { retry_in, .. } = next(&mut events).await else { panic!() };
        assert_eq!(retry_in, Duration::from_secs(22));

        // A failed connect keeps doubling; a connection that subscribed resets the counter.
        connector.refuse(ConnectError::Failed("dns".into()));
        assert_eq!(next(&mut events).await, XivaEvent::Connecting);
        let XivaEvent::Disconnected { retry_in, .. } = next(&mut events).await else { panic!() };
        assert_eq!(retry_in, Duration::from_secs(44));
        let third = connector.accept();
        assert_eq!(next(&mut events).await, XivaEvent::Connecting);
        third.subscribed("s3");
        assert!(matches!(next(&mut events).await, XivaEvent::Connected { .. }));
        third.close("bye");
        let XivaEvent::Disconnected { retry_in, .. } = next(&mut events).await else { panic!() };
        assert_eq!(retry_in, Duration::from_secs(11));
        assert_eq!(connector.targets.lock().unwrap().len(), 4);
    }

    #[test]
    fn ping_interval_is_bounded() {
        assert_eq!(ping_deadline(Some(10.0)), Duration::from_secs(13));
        for bad in [None, Some(-1.0), Some(0.0), Some(1e308), Some(f64::NAN), Some(f64::INFINITY)] {
            assert_eq!(ping_deadline(bad), Duration::from_secs(78), "{bad:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn hostile_ping_intervals_do_not_kill_the_actor() {
        let connector = FakeConnector::default();
        let mut server = connector.accept();
        let (handle, mut events) = start(&connector);
        next(&mut events).await;
        server.text(json!({ "operation": "ping", "server-interval-sec": -1 }));
        server.text(json!({ "operation": "ping", "server-interval-sec": 1e308 }));
        let h = handle.clone();
        let task = tokio::spawn(async move { h.chat_list(&[]).await });
        let (req_id, _, body) = server.next_request().await;
        server.reply(req_id, "history", json!({ "RequestId": body["RequestId"] }));
        assert_eq!(task.await.unwrap().unwrap().len(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn silent_connection_is_dropped_by_the_ping_watchdog() {
        let connector = FakeConnector::default();
        let _server = connector.accept();
        let (_handle, mut events) = start(&connector);
        next(&mut events).await;
        let XivaEvent::Disconnected { reason, .. } = next(&mut events).await else { panic!() };
        assert!(reason.contains("ping"));
    }

    #[tokio::test(start_paused = true)]
    async fn offline_requests_fail_fast_and_pending_ones_fail_on_drop() {
        let connector = FakeConnector::default();
        let mut server = connector.accept();
        let (handle, mut events) = start(&connector);
        next(&mut events).await;
        let h = handle.clone();
        let task = tokio::spawn(async move { h.chat_list(&[]).await });
        server.next_request().await;
        server.close("net");
        assert!(matches!(task.await.unwrap(), Err(ProtoError::Network(m)) if m.contains("connection lost")));
        assert!(matches!(next(&mut events).await, XivaEvent::Disconnected { .. }));
        yield_now().await;
        assert_eq!(handle.chat_list(&[]).await, Err(ProtoError::Network("not connected".into())));
    }

    #[tokio::test(start_paused = true)]
    async fn handshake_rejection_expires_the_session_and_stops_reconnecting() {
        let connector = FakeConnector::default();
        connector.refuse(ConnectError::Unauthorized);
        let (handle, mut events) = start(&connector);
        assert_eq!(next(&mut events).await, XivaEvent::Connecting);
        assert_eq!(next(&mut events).await, XivaEvent::SessionExpired);
        assert_eq!(handle.chat_list(&[]).await, Err(ProtoError::SessionExpired));
        assert_eq!(connector.targets.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn auth_close_reason_expires_the_session() {
        let connector = FakeConnector::default();
        let mut server = connector.accept();
        let (handle, mut events) = start(&connector);
        next(&mut events).await;
        let h = handle.clone();
        let task = tokio::spawn(async move { h.chat_list(&[]).await });
        server.next_request().await;
        server.close("Cookie auth failed");
        assert_eq!(task.await.unwrap(), Err(ProtoError::SessionExpired));
        assert_eq!(next(&mut events).await, XivaEvent::SessionExpired);
        assert_eq!(connector.targets.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn watched_chats_are_subscribed_after_every_connect() {
        let connector = FakeConnector::default();
        let mut first = connector.accept();
        let (handle, mut events) = start(&connector);
        next(&mut events).await;
        handle.watch_chats(vec!["c1".into(), "c2".into()]);
        yield_now().await;
        first.subscribed("sub1");
        next(&mut events).await;
        for chat in ["c1", "c2"] {
            let (_, method, body) = first.next_request().await;
            assert_eq!(method, "subscribe");
            assert_eq!(body["ChatId"], chat);
            assert_eq!(body["ClientTransportId"]["XivaSubscriptionId"], "sub1");
        }
        first.close("x");
        let mut second = connector.accept();
        while !matches!(next(&mut events).await, XivaEvent::Connecting) {}
        second.subscribed("sub2");
        let (_, _, body) = second.next_request().await;
        assert_eq!(body["ClientTransportId"]["XivaSubscriptionId"], "sub2");
    }

    #[tokio::test(start_paused = true)]
    async fn rejected_subscription_expires_the_session() {
        let connector = FakeConnector::default();
        let mut server = connector.accept();
        let (handle, mut events) = start(&connector);
        next(&mut events).await;
        handle.watch_chats(vec!["c1".into()]);
        yield_now().await;
        server.subscribed("sub1");
        next(&mut events).await;
        let (req_id, _, _) = server.next_request().await;
        server.reply(req_id, "subscribe", json!({ "Status": 6 }));
        assert_eq!(next(&mut events).await, XivaEvent::SessionExpired);
        assert_eq!(handle.chat_list(&[]).await, Err(ProtoError::SessionExpired));
        assert_eq!(connector.targets.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn trace_level_shows_operation_frames() {
        let capture = Capture::default();
        let _guard = capture.install();
        let connector = FakeConnector::default();
        let server = connector.accept();
        let (_handle, mut events) = start(&connector);
        next(&mut events).await;
        server.subscribed("s-1");
        assert!(matches!(next(&mut events).await, XivaEvent::Connected { .. }));
        let text = capture.text();
        assert!(text.contains("operation frame") && text.contains("subscription-id"), "{text}");
        assert!(text.contains("subscribed") && text.contains("s-1"), "{text}");
    }

    #[tokio::test(start_paused = true)]
    async fn debug_level_logs_push_kind_without_message_text() {
        let capture = Capture::default();
        let _guard = capture.install_at(tracing::Level::DEBUG);
        let connector = FakeConnector::default();
        let server = connector.accept();
        let (_handle, mut events) = start(&connector);
        next(&mut events).await;
        server.push_frame(json!({
            "ClientMessage": { "Plain": { "ChatId": "chat-9", "Text": { "MessageText": "TOPSECRET-TEXT" } } },
            "ServerMessageInfo": { "Timestamp": 5 }
        }));
        assert!(matches!(next(&mut events).await, XivaEvent::Push(PushEvent::MessageUpserted(_))));
        let text = capture.text();
        assert!(text.contains("MessageUpserted") && text.contains("chat-9"), "{text}");
        capture.assert_absent(&["TOPSECRET-TEXT", "operation frame"]);
    }

    #[tokio::test(start_paused = true)]
    async fn subscribe_round_is_summed_up_in_one_line() {
        let capture = Capture::default();
        let _guard = capture.install_at(tracing::Level::DEBUG);
        let connector = FakeConnector::default();
        let mut server = connector.accept();
        let (handle, mut events) = start(&connector);
        next(&mut events).await;
        handle.watch_chats(vec!["c1".into(), "c2".into(), "c3".into()]);
        yield_now().await;
        server.subscribed("sub1");
        next(&mut events).await;
        let (first, _, _) = server.next_request().await;
        let (second, _, _) = server.next_request().await;
        let (third, _, _) = server.next_request().await;
        server.reply(first, "subscribe", json!({}));
        server.proxy_status(second, 7);
        server.proxy_status(third, 7);
        // The actor handles frames in order: once this reply is back the round is over.
        let h = handle.clone();
        let task = tokio::spawn(async move { h.chat_list(&[]).await });
        let (req_id, _, body) = server.next_request().await;
        server.reply(req_id, "history", json!({ "RequestId": body["RequestId"] }));
        task.await.unwrap().unwrap();
        let rounds: Vec<String> = capture.text().lines().filter(|l| l.contains("subscribe round")).map(str::to_owned).collect();
        assert_eq!(rounds.len(), 1, "{rounds:?}");
        let line = &rounds[0];
        assert!(line.contains("WARN") && line.contains("chats=3") && line.contains("ok=1") && line.contains("refused=2"), "{line}");
        assert!(line.contains("TOO_MANY_REQUESTS=2"), "{line}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_fully_accepted_subscribe_round_is_a_debug_line() {
        let capture = Capture::default();
        let _guard = capture.install_at(tracing::Level::DEBUG);
        let connector = FakeConnector::default();
        let mut server = connector.accept();
        let (handle, mut events) = start(&connector);
        next(&mut events).await;
        handle.watch_chats(vec!["c1".into(), "c2".into()]);
        yield_now().await;
        server.subscribed("sub1");
        next(&mut events).await;
        for _ in 0..2 {
            let (req_id, _, _) = server.next_request().await;
            server.reply(req_id, "subscribe", json!({}));
        }
        let h = handle.clone();
        let task = tokio::spawn(async move { h.chat_list(&[]).await });
        let (req_id, _, body) = server.next_request().await;
        server.reply(req_id, "history", json!({ "RequestId": body["RequestId"] }));
        task.await.unwrap().unwrap();
        let text = capture.text();
        let round = text.lines().find(|l| l.contains("subscribe round")).expect("round line");
        assert!(round.contains("DEBUG") && round.contains("chats=2") && round.contains("ok=2"), "{round}");
    }

    #[tokio::test(start_paused = true)]
    async fn log_never_contains_cookie_or_url_query_values() {
        let capture = Capture::default();
        let _guard = capture.install();
        let connector = FakeConnector::default();
        let mut server = connector.accept();
        let cookie = SessionCookie { value: "c00kie-VALUE".into(), ..cookie() };
        let (handle, mut events) = XivaHandle::spawn(connector.clone(), XivaConfig::production(), "user-q-7f3a".into(), vec![cookie]);
        next(&mut events).await;
        server.subscribed("sub-1");
        next(&mut events).await;
        let h = handle.clone();
        let task = tokio::spawn(async move { h.history("c1", None, 10).await });
        let (req_id, _, body) = server.next_request().await;
        server.reply(req_id, "history", json!({ "RequestId": body["RequestId"] }));
        assert!(task.await.unwrap().is_ok());
        server.push_frame(json!({ "ClientMessage": { "Typing": { "ChatId": "c1" } } }));
        server.close("bye");
        let XivaEvent::Disconnected { .. } = next(&mut events).await else { panic!() };

        let target = connector.targets.lock().unwrap()[0].clone();
        let session = target.url.split("session=").nth(1).and_then(|s| s.split('&').next()).unwrap().to_owned();
        let text = capture.text();
        for needle in ["connecting", "push.yandex.ru/v2/subscribe/websocket", "socket request", "socket reply", "disconnected"] {
            assert!(text.contains(needle), "{needle}: {text}");
        }
        capture.assert_absent(&["c00kie-VALUE", "user-q-7f3a", &session, "?service", "client=web_main"]);
    }
}
