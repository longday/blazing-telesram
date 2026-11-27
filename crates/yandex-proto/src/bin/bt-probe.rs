//! Developer probe for the Yandex protocol layer. Reads, plus a few writes that act on the
//! account's self chat only (send, edit, delete, typing, and `react-self`, which sets, replaces or
//! removes the account's own reaction on a self-chat message); never prints cookie values.
//! `listen` prints reaction pushes through `Debug` like every other push and `history` lists the
//! reactions of a message after its text. `download` saves one file, image or voice message
//! of any chat into a local directory (never overwriting) and prints host and path of the
//! request, never its query. `reactors` lists who reacted to one message (a read, covered by no
//! write guard). `create-private` and `create-chat` are the commands that change the
//! account outside the self chat, and they send nothing without `--confirm-create`.
//! Every command that sends a write (`send-self`, `edit-self`, `delete-self`, `react-self`,
//! `typing-self`, `seen`, `typing`, `create-private`, `create-chat`) obeys `BT_WRITE_ONLY_CHAT` like
//! the desktop app: set, it limits writes to the chat it names, and it refuses every creation.
//! Command output goes to stdout; diagnostics go through `tracing` to stderr (filter: `BT_LOG`).

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::Duration;

use tracing::error;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use yandex_proto::bootstrap::{RegistryEndpoints, bootstrap};
use yandex_proto::media::{MediaClient, MediaConfig, MediaTarget};
use yandex_proto::request::{MEMBERS_PAGE, REACTORS_PAGE, SEARCH_LIMIT, SearchEntity};
use yandex_proto::runtime::RuntimeDir;
use yandex_proto::search::SearchSet;
use yandex_proto::session::Session;
use yandex_proto::wire::{
    Relation, WireBody, WireChat, WireFile, WireMessage, without_refusal_reason,
};
use yandex_proto::write::{Committed, ReactionChange, ReplyRef, WriteRequest};
use yandex_proto::xiva::{XivaEvent, XivaEvents};
use yandex_proto::{Client, ClientConfig, MAX_CHAT_TITLE, ManageError, ProtoError};

const USAGE: &str = "usage: bt-probe me | chats | history <chat> [--before <micros>] | listen [--watch <chat>]
       bt-probe send-self <text> [--payload <id>] [--reply <micros>] [--chat <self chat>]
       bt-probe edit-self <micros> <text> [--reply <micros>] [--important] [--chat <self chat>]
       bt-probe delete-self <micros> [--chat <self chat>]
       bt-probe react-self <micros> <code|remove> [--chat <self chat>]
       bt-probe typing-self [--chat <self chat>] | seen <chat> <micros> | typing <chat>
       bt-probe download <chat> <micros> [--item N] [--to <dir>] [--url original|plain|stream]
       bt-probe search <query> [--entities messages,users,chats] [--limit <n>]
       bt-probe whoami | members <chat>
       bt-probe reactors <chat> <micros> [--before <micros>] [--limit <n>]
       bt-probe create-private <guid> --confirm-create
       bt-probe create-chat <title> <guid>... --confirm-create
the *-self commands refuse any chat but the account's own self chat
BT_WRITE_ONLY_CHAT limits every write command to the chat it names: a write to another chat, a set but empty value, and create-private / create-chat (a new chat is never the named one) are refused with exit code 2 before any connection or request
download takes the file, voice message or the N-th (from 1) image of the message with that timestamp, saves it
into <dir> (default: the current directory) and refuses to overwrite an existing file
download --url is the playback research request: it asks for one URL variant (original: file_shortterm/{id}?attach=true, plain: file_shortterm/{id}, stream: video_streams/{id}), needs --to <dir> that already exists with mode 0700 (the sample directory: the bytes are message content, nothing else is saved under a name that shows them) and prints only status, landed host, content-type, content-length, accept-ranges, whether content-disposition is present, bytes received and sha256; a non-2xx answer reads no body, prints the same lines and exits 1
log filter: BT_LOG (default info), e.g. BT_LOG=yandex_proto=debug or BT_LOG=yandex_proto::xiva=trace
search is read-only: per entity it prints total/limit/page/pages and the raw items, one JSON per line (the query is not echoed or logged above trace; items carry names and message text)
react-self puts the reaction <code> (a positive integer, the web client's reaction type) on the self-chat message with that timestamp, or takes the account's reaction away with `remove`; it sends one write push and prints the same status line as the other *-self commands. history lists a row's reactions after its text: code x count, the version, the recent reactors (user:code)
whoami, members and reactors are read-only and print the parsed reply (`{:#?}`) on stdout (guids and display names; the header line goes to stderr). members asks get_chat_members for the first page of 50 users and answers group chats only
reactors is read-only (a socket list_reactions, so BT_WRITE_ONLY_CHAT does not apply): it prints one page of the users who reacted to the message with that timestamp in <chat>, in server order; --before <micros> asks for the page older than that cursor, --limit <n> (at least 1, default 30) sets the page size. It always sends Mode 0; there is no --mode
chats prints `pinned=<micros>` after the unread count of a chat that has a pinned message (the message's timestamp); a chat without a pin prints nothing extra
edit-self rewrites the whole message: --important adds `IsImportant: true` to it (off by default, so the default frame is the plain edit's) for a message that is marked important, to check whether an edit without the flag clears it; it never marks a message by itself
create-private opens (creating it when missing) the private chat with the user <guid> on the real account, once and never retried; without --confirm-create it prints the target and exits with code 2 without any request. Exit codes: 0 created, 1 not sent or rejected (nothing was created), 3 outcome unknown (the chat may exist: check the web client before repeating)
create-chat creates a group chat named <title> with the listed participants (guids, not your own) on the real account, once and never retried; the payload is the web client's own (description empty, not a channel, is_public true), not yet confirmed against the server. Without --confirm-create it prints the title and participants it would send and exits with code 2 without any request; the title must be non-blank and at most 250 characters, every guid distinct. Same exit codes as create-private; a created group whose reply lists participants the server could not add still exits 0 and names them";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

type Failure = (u8, String);

/// Installs the stderr logger; the error names `BT_LOG` when its value is not a filter.
fn init_logging() -> Result<(), String> {
    tracing_subscriber::registry()
        .with(yandex_proto::log::filter_from_env()?)
        .with(yandex_proto::log::dependency_cap())
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(std::io::stderr().is_terminal()),
        )
        .try_init()
        .map_err(|e| format!("cannot install the logger: {e}"))
}

fn main() -> ExitCode {
    if let Err(message) = init_logging() {
        eprintln!("bt-probe: {message}");
        return ExitCode::from(2);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["me"] => block_on(me()),
        ["chats"] => block_on(chats()),
        ["history", chat] => block_on(history(chat, None)),
        ["history", chat, "--before", before] => match before.parse() {
            Ok(before) => block_on(history(chat, Some(before))),
            Err(_) => return usage(),
        },
        ["listen"] => block_on(listen(None)),
        ["listen", "--watch", chat] => block_on(listen(Some(chat))),
        ["send-self", text, flags @ ..] => match send_flags(flags) {
            Some(flags) => block_on(send_self(text, flags)),
            None => return usage(),
        },
        ["edit-self", ts, text, flags @ ..] => match (ts.parse(), edit_flags(flags)) {
            (Ok(ts), Some(flags)) => block_on(edit_self(ts, text, flags)),
            _ => return usage(),
        },
        ["delete-self", ts, flags @ ..] => match (ts.parse(), chat_flag(flags)) {
            (Ok(ts), Some(chat)) => block_on(write_self(chat, move |chat| WriteRequest::Delete {
                chat,
                timestamp: ts,
            })),
            _ => return usage(),
        },
        ["react-self", ts, change, flags @ ..] => {
            match (ts.parse(), reaction_change(change), chat_flag(flags)) {
                (Ok(ts), Some(change), Some(chat)) => {
                    block_on(write_self(chat, move |chat| WriteRequest::Reaction {
                        chat,
                        timestamp: ts,
                        change,
                    }))
                }
                _ => return usage(),
            }
        }
        ["typing-self", flags @ ..] => match chat_flag(flags) {
            Some(chat) => block_on(write_self(chat, |chat| WriteRequest::Typing { chat })),
            None => return usage(),
        },
        ["seen", chat, ts] => match ts.parse() {
            Ok(ts) => block_on(write_named(
                chat,
                WriteRequest::Seen {
                    chat: (*chat).to_owned(),
                    timestamp: ts,
                },
            )),
            Err(_) => return usage(),
        },
        ["typing", chat] => block_on(write_named(
            chat,
            WriteRequest::Typing {
                chat: (*chat).to_owned(),
            },
        )),
        ["download", chat, ts, flags @ ..] => match (ts.parse(), download_flags(flags)) {
            (Ok(ts), Some(flags)) => block_on(download(chat, ts, flags)),
            _ => return usage(),
        },
        ["search", query, flags @ ..] => match search_flags(flags) {
            Some(flags) => block_on(search(query, flags)),
            None => return usage(),
        },
        ["whoami"] => block_on(whoami()),
        ["members", chat] => block_on(members(chat)),
        ["reactors", rest @ ..] => match reactors_args(rest) {
            Some(args) => block_on(reactors(args)),
            None => return usage(),
        },
        ["create-private", guid, flags @ ..] => match create_private_args(guid, flags) {
            Some(CreatePrivate::Send(guid)) => block_on(create_private(guid)),
            Some(CreatePrivate::Refuse(guid)) => {
                eprintln!("{}", refusal(guid));
                return ExitCode::from(2);
            }
            None => return usage(),
        },
        ["create-chat", title, rest @ ..] => match create_chat_args(title, rest) {
            Some(plan) if plan.confirmed => block_on(create_chat(plan)),
            Some(plan) => {
                eprintln!("{}", group_refusal(&plan));
                return ExitCode::from(2);
            }
            None => return usage(),
        },
        _ => return usage(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, message)) => {
            error!("{message}");
            ExitCode::from(code)
        }
    }
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

fn block_on(future: impl Future<Output = Result<(), Failure>>) -> Result<(), Failure> {
    tokio::runtime::Runtime::new()
        .map_err(|e| (1, e.to_string()))?
        .block_on(future)
}

/// A failure as `main` logs it and the commands print it. A refused status keeps its code and
/// name but not the server's reason (`ErrorInfo.Message`); the raw reply is at
/// `BT_LOG=yandex_proto::xiva=trace` for a developer who wants it.
fn fail(e: ProtoError) -> Failure {
    match e {
        ProtoError::SessionExpired => (1, "session expired: sign in again via ./run.sh".to_owned()),
        ProtoError::Protocol(text) => (
            1,
            ProtoError::Protocol(without_refusal_reason(&text).into()).to_string(),
        ),
        other => (1, other.to_string()),
    }
}

fn load_session() -> Result<Session, Failure> {
    let dir = RuntimeDir::from_env().map_err(|e| (2, e.to_string()))?;
    Session::load(&dir)
        .map_err(|e| (1, e.to_string()))?
        .ok_or_else(|| {
            (
                1,
                "no stored session: sign in first via ./run.sh".to_owned(),
            )
        })
}

/// The account's chat with itself: `<guid>_<guid>`.
fn self_chat(guid: &str) -> String {
    format!("{guid}_{guid}")
}

/// The chat a `*-self` command acts on. `--chat` exists only so that naming another chat is an
/// error instead of a silent redirect.
fn resolve_self_chat(requested: Option<&str>, guid: &str) -> Result<String, String> {
    let own = self_chat(guid);
    match requested {
        Some(chat) if chat != own => Err(format!(
            "refusing {chat}: the *-self commands act on the self chat only"
        )),
        _ => Ok(own),
    }
}

/// `[--chat <id>]` or nothing; `None` for anything else.
fn chat_flag<'a>(flags: &[&'a str]) -> Option<Option<&'a str>> {
    match flags {
        [] => Some(None),
        ["--chat", chat] => Some(Some(chat)),
        _ => None,
    }
}

/// The `<code|remove>` argument of `react-self`: `remove`, or a positive integer reaction type
/// (the wire carries `Type` as an integer); anything else is a usage error.
fn reaction_change(arg: &str) -> Option<ReactionChange> {
    if arg == "remove" {
        return Some(ReactionChange::Remove);
    }
    arg.parse::<i64>()
        .ok()
        .filter(|code| *code > 0)
        .map(ReactionChange::Set)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SendFlags<'a> {
    payload: Option<&'a str>,
    reply: Option<i64>,
    chat: Option<&'a str>,
}

fn send_flags<'a>(mut flags: &[&'a str]) -> Option<SendFlags<'a>> {
    let mut parsed = SendFlags::default();
    while let [name, value, rest @ ..] = flags {
        match *name {
            "--payload" => parsed.payload = Some(value),
            "--reply" => parsed.reply = Some(value.parse().ok()?),
            "--chat" => parsed.chat = Some(value),
            _ => return None,
        }
        flags = rest;
    }
    flags.is_empty().then_some(parsed)
}

/// The flags of `edit-self`: those of a send minus the payload id, and `--important`. Without
/// `--reply` an edit repeats the reply the message already has; `--reply` re-attaches one to a
/// message that lost it.
#[derive(Debug, Default, PartialEq, Eq)]
struct EditFlags<'a> {
    send: SendFlags<'a>,
    important: bool,
}

fn edit_flags<'a>(mut flags: &[&'a str]) -> Option<EditFlags<'a>> {
    let mut parsed = EditFlags::default();
    loop {
        match flags {
            [] => return Some(parsed),
            ["--important", rest @ ..] => {
                parsed.important = true;
                flags = rest;
            }
            [name, value, rest @ ..] => {
                match *name {
                    "--reply" => parsed.send.reply = Some(value.parse().ok()?),
                    "--chat" => parsed.send.chat = Some(value),
                    _ => return None,
                }
                flags = rest;
            }
            _ => return None,
        }
    }
}

fn print_committed(committed: &Committed) {
    let outcome = if committed.duplicate() {
        "DUPLICATE"
    } else {
        "FULLY_COMMITTED"
    };
    print!("status={} {outcome}", committed.status);
    if let Some(info) = &committed.message {
        print!(
            " timestamp={} seq_no={:?} version={:?}",
            info.timestamp_mcs, info.seq_no, info.version
        );
    }
    println!();
}

async fn send_self(text: &str, flags: SendFlags<'_>) -> Result<(), Failure> {
    let allowed = write_only()?;
    let session = load_session()?;
    let chat = guarded_self_chat(flags.chat, &session.identity.guid, allowed.as_deref())?;
    let (client, _events) = connect(&session, ClientConfig::production()).await?;
    let reply = match flags.reply {
        Some(timestamp) => Some(reply_ref(&client, &chat, timestamp).await?),
        None => None,
    };
    let payload_id = flags
        .payload
        .map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
    println!("payload: {payload_id}");
    let request = WriteRequest::SendText {
        chat,
        payload_id,
        text: text.to_owned(),
        reply,
    };
    print_committed(&client.push(request).await.map_err(fail)?);
    Ok(())
}

/// A message of the chat, from the page that ends with it (`before` is exclusive).
async fn find_message(client: &Client, chat: &str, timestamp: i64) -> Result<WireMessage, Failure> {
    let page = client
        .history(chat, Some(timestamp + 1))
        .await
        .map_err(fail)?;
    page.messages
        .into_iter()
        .find(|m| m.timestamp == timestamp)
        .ok_or_else(|| (1, format!("no message {timestamp} in {chat}")))
}

/// A new reply's target as the composer would hold it: its timestamp and its current text.
async fn reply_ref(client: &Client, chat: &str, timestamp: i64) -> Result<ReplyRef, Failure> {
    let original = find_message(client, chat, timestamp).await?;
    let WireBody::Text(quote) = original.body else {
        return Err((1, format!("message {timestamp} has no text to quote")));
    };
    Ok(ReplyRef { timestamp, quote })
}

/// The reply an edited message already has, quote included as displayed. The original may have
/// been edited or deleted since, so it is not read again.
fn stored_reply(message: &WireMessage) -> Result<Option<ReplyRef>, String> {
    let timestamp = message.timestamp;
    match &message.relation {
        Relation::None => Ok(None),
        Relation::Forward { .. } => Err(format!(
            "message {timestamp} is a forward; editing it would drop the forward"
        )),
        Relation::Reply {
            quote,
            reference,
            original,
        } => {
            let target = reference
                .as_ref()
                .and_then(|r| r.timestamp)
                .or_else(|| original.as_ref()?.timestamp);
            let quote = quote.clone().or_else(|| match &original.as_ref()?.body {
                WireBody::Text(text) => Some(text.clone()),
                _ => None,
            });
            match (target, quote) {
                (Some(timestamp), Some(quote)) => Ok(Some(ReplyRef { timestamp, quote })),
                _ => Err(format!(
                    "the reply of message {timestamp} has no known target or quote; pass --reply"
                )),
            }
        }
    }
}

async fn edit_self(timestamp: i64, text: &str, flags: EditFlags<'_>) -> Result<(), Failure> {
    let EditFlags {
        send: flags,
        important,
    } = flags;
    let allowed = write_only()?;
    let session = load_session()?;
    let chat = guarded_self_chat(flags.chat, &session.identity.guid, allowed.as_deref())?;
    let (client, _events) = connect(&session, ClientConfig::production()).await?;
    let reply = match flags.reply {
        Some(target) => Some(reply_ref(&client, &chat, target).await?),
        None => {
            stored_reply(&find_message(&client, &chat, timestamp).await?).map_err(|e| (1, e))?
        }
    };
    print_committed(
        &client
            .push(WriteRequest::Edit {
                chat,
                timestamp,
                text: text.to_owned(),
                reply,
                important,
            })
            .await
            .map_err(fail)?,
    );
    Ok(())
}

/// The self chat a `*-self` write goes to: `resolve_self_chat`, then the `BT_WRITE_ONLY_CHAT`
/// guard. Runs before the connection is made, so a refusal sends nothing.
fn guarded_self_chat(
    requested: Option<&str>,
    guid: &str,
    allowed: Option<&str>,
) -> Result<String, Failure> {
    let chat = resolve_self_chat(requested, guid).map_err(|e| (2, e))?;
    chat_write_guard(allowed, &chat).map_err(|e| (2, e))?;
    Ok(chat)
}

/// Delete, typing and reaction: one push into the self chat.
async fn write_self(
    requested: Option<&str>,
    build: impl FnOnce(String) -> WriteRequest,
) -> Result<(), Failure> {
    let allowed = write_only()?;
    let session = load_session()?;
    let chat = guarded_self_chat(requested, &session.identity.guid, allowed.as_deref())?;
    let (client, _events) = connect(&session, ClientConfig::production()).await?;
    print_committed(&client.push(build(chat)).await.map_err(fail)?);
    Ok(())
}

/// Read marker or typing in any chat; the caller names the chat on purpose. Neither carries text.
/// `chat` is the chat `request` is addressed to, for the `BT_WRITE_ONLY_CHAT` guard.
async fn write_named(chat: &str, request: WriteRequest) -> Result<(), Failure> {
    let allowed = write_only()?;
    chat_write_guard(allowed.as_deref(), chat).map_err(|e| (2, e))?;
    let session = load_session()?;
    let (client, _events) = connect(&session, ClientConfig::production()).await?;
    print_committed(&client.push(request).await.map_err(fail)?);
    Ok(())
}

const WRITE_ONLY_VAR: &str = "BT_WRITE_ONLY_CHAT";

/// `BT_WRITE_ONLY_CHAT` as the desktop app reads it: unset means no limit; set, it must name a
/// chat (a blank value is an error, not "no limit").
fn parse_write_only(value: Option<String>) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(chat) if chat.trim().is_empty() => Err(format!("{WRITE_ONLY_VAR} is set but empty")),
        Some(chat) => Ok(Some(chat)),
    }
}

/// The limit from the environment; a usage-class failure (exit 2) when the value is unusable.
/// A value that is not valid unicode cannot name a chat, so it refuses instead of lifting the limit.
fn write_only() -> Result<Option<String>, Failure> {
    let value = match std::env::var(WRITE_ONLY_VAR) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err((2, format!("{WRITE_ONLY_VAR} is not valid unicode")));
        }
    };
    parse_write_only(value).map_err(|e| (2, e))
}

/// A write into `chat` is allowed when no limit is set or the limit names that chat.
fn chat_write_guard(allowed: Option<&str>, chat: &str) -> Result<(), String> {
    match allowed {
        Some(allowed) if allowed != chat => Err(format!(
            "refusing a write to {chat}: {WRITE_ONLY_VAR} limits writes to {allowed}; nothing was sent"
        )),
        _ => Ok(()),
    }
}

/// Creating a chat writes outside any one chat and a new chat cannot be the named one, so any
/// limit refuses it, whatever the target.
fn creation_guard(allowed: Option<&str>) -> Result<(), Failure> {
    match allowed {
        Some(chat) => Err((
            2,
            format!(
                "refusing to create a chat: {WRITE_ONLY_VAR} limits writes to {chat}, and a new chat can never be that chat; nothing was sent"
            ),
        )),
        None => Ok(()),
    }
}

async fn me() -> Result<(), Failure> {
    let session = load_session()?;
    let identity = bootstrap(&RegistryEndpoints::production(), &session.cookies)
        .await
        .map_err(|e| (1, e.to_string()))?;
    println!("uid: {}", identity.uid.as_deref().unwrap_or("-"));
    println!("guid: {}", identity.guid);
    println!(
        "display name: {}",
        identity.display_name.as_deref().unwrap_or("-")
    );
    Ok(())
}

/// Starts the client and waits until the push socket is subscribed, so requests do not fail offline.
async fn connect(session: &Session, config: ClientConfig) -> Result<(Client, XivaEvents), Failure> {
    let (client, mut events) = Client::start(session, config).map_err(fail)?;
    let wait = async {
        loop {
            match events.recv().await {
                Some(XivaEvent::Connected { subscription_id }) => {
                    println!("connected, subscription-id: {subscription_id}");
                    return Ok(());
                }
                Some(XivaEvent::SessionExpired) => return Err(fail(ProtoError::SessionExpired)),
                Some(_) => {}
                None => return Err((1, "connection task stopped".to_owned())),
            }
        }
    };
    tokio::time::timeout(CONNECT_TIMEOUT, wait)
        .await
        .map_err(|_| (1, "timed out waiting for the push socket".to_owned()))??;
    Ok((client, events))
}

async fn chats() -> Result<(), Failure> {
    let (client, _events) = connect(&load_session()?, ClientConfig::production()).await?;
    for chat in client.chat_list().await.map_err(fail)? {
        println!("{}", chat_line(&chat));
    }
    Ok(())
}

/// One `chats` row: id, kind, title, unread (with the muted flag and, for a chat that has one,
/// the pinned message's timestamp), activity timestamp, last message.
fn chat_line(chat: &WireChat) -> String {
    let preview = chat.last_message.as_ref().map(describe).unwrap_or_default();
    format!(
        "{}\t{:?}\t{}\tunread={}{}{}\tts={}\t{preview}",
        chat.id,
        chat.kind,
        chat.title.as_deref().unwrap_or("-"),
        chat.unread,
        if chat.muted { " muted" } else { "" },
        chat.pinned
            .map_or_else(String::new, |t| format!(" pinned={t}")),
        chat.last_activity
            .map_or_else(|| "-".to_owned(), |t| t.to_string()),
    )
}

async fn history(chat: &str, before: Option<i64>) -> Result<(), Failure> {
    let (client, _events) = connect(&load_session()?, ClientConfig::production()).await?;
    let page = client.history(chat, before).await.map_err(fail)?;
    for message in &page.messages {
        println!("{}", describe(message));
    }
    println!(
        "-- {} messages, reached_start={}, skipped={}",
        page.messages.len(),
        page.reached_start,
        page.skipped
    );
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct SearchFlags {
    entities: Vec<SearchEntity>,
    limit: u32,
}

/// `[--entities messages,users,chats] [--limit <n>]` in any order, each at most once. `contacts`
/// and every other name are refused (the request type cannot express them); a limit is at least 1.
fn search_flags(mut flags: &[&str]) -> Option<SearchFlags> {
    let (mut entities, mut limit) = (None, None);
    while let [name, value, rest @ ..] = flags {
        match *name {
            "--entities" if entities.is_none() => entities = Some(search_entities(value)?),
            "--limit" if limit.is_none() => {
                limit = Some(value.parse::<u32>().ok().filter(|n| *n > 0)?)
            }
            _ => return None,
        }
        flags = rest;
    }
    flags.is_empty().then(|| SearchFlags {
        entities: entities.unwrap_or_else(|| {
            vec![
                SearchEntity::Messages,
                SearchEntity::Users,
                SearchEntity::Chats,
            ]
        }),
        limit: limit.unwrap_or(SEARCH_LIMIT),
    })
}

/// A comma-separated, duplicate-free list of entity names.
fn search_entities(list: &str) -> Option<Vec<SearchEntity>> {
    let mut entities = Vec::new();
    for name in list.split(',') {
        let entity = [
            SearchEntity::Messages,
            SearchEntity::Users,
            SearchEntity::Chats,
        ]
        .into_iter()
        .find(|e| e.wire_name() == name)?;
        if entities.contains(&entity) {
            return None;
        }
        entities.push(entity);
    }
    Some(entities)
}

fn print_search_set(entity: SearchEntity, set: &SearchSet) {
    println!(
        "{}: total={} limit={} page={} pages={}",
        entity.wire_name(),
        set.total,
        set.limit,
        set.page,
        set.pages
    );
    for item in &set.items {
        println!("{item}");
    }
}

/// One registry `search` (HTTP, bare cookie): the first page of every requested entity.
async fn search(query: &str, flags: SearchFlags) -> Result<(), Failure> {
    let (client, _events) =
        Client::start(&load_session()?, ClientConfig::production()).map_err(fail)?;
    let reply = client
        .search_with(query, flags.limit, &flags.entities)
        .await
        .map_err(fail)?;
    for entity in flags.entities {
        let set = match entity {
            SearchEntity::Messages => &reply.messages,
            SearchEntity::Users => &reply.users,
            SearchEntity::Chats => &reply.chats,
        };
        print_search_set(entity, set);
    }
    Ok(())
}

/// The signed-in account's own `whoami` reply, parsed (guid, name, avatar id, status text).
async fn whoami() -> Result<(), Failure> {
    let (client, _events) = connect(&load_session()?, ClientConfig::production()).await?;
    println!("{:#?}", client.whoami().await.map_err(fail)?);
    Ok(())
}

/// The first page (50 users) of the members of one group chat (`get_chat_members`, registry,
/// bare cookie), parsed.
async fn members(chat: &str) -> Result<(), Failure> {
    let (client, _events) =
        Client::start(&load_session()?, ClientConfig::production()).map_err(fail)?;
    eprintln!("members of {chat} via get_chat_members");
    println!(
        "{:#?}",
        client
            .members(chat, MEMBERS_PAGE, None)
            .await
            .map_err(fail)?
    );
    Ok(())
}

/// What `reactors <chat> <micros> [flags]` asks for.
#[derive(Debug, PartialEq, Eq)]
struct ReactorsArgs {
    chat: String,
    timestamp: i64,
    before: Option<i64>,
    limit: u32,
}

/// The arguments after `reactors`: `<chat> <micros> [--before <micros>] [--limit <n>]`, the flags
/// in any order and each at most once; a limit is at least 1. A missing timestamp, a malformed
/// number or any other flag (there is no `--mode`: the request form is fixed) is `None`, a usage
/// error.
fn reactors_args(args: &[&str]) -> Option<ReactorsArgs> {
    let [chat, timestamp, flags @ ..] = args else {
        return None;
    };
    let timestamp = timestamp.parse::<i64>().ok()?;
    let (mut before, mut limit) = (None, None);
    let mut flags = flags;
    while let [name, value, rest @ ..] = flags {
        match *name {
            "--before" if before.is_none() => before = Some(value.parse::<i64>().ok()?),
            "--limit" if limit.is_none() => {
                limit = Some(value.parse::<u32>().ok().filter(|n| *n > 0)?)
            }
            _ => return None,
        }
        flags = rest;
    }
    flags.is_empty().then(|| ReactorsArgs {
        chat: (*chat).to_owned(),
        timestamp,
        before,
        limit: limit.unwrap_or(REACTORS_PAGE),
    })
}

/// One page of the users who reacted to a message (socket `list_reactions`, `Mode` 0), parsed.
/// A read: nothing is written, so `BT_WRITE_ONLY_CHAT` does not apply.
async fn reactors(args: ReactorsArgs) -> Result<(), Failure> {
    let (client, _events) = connect(&load_session()?, ClientConfig::production()).await?;
    eprintln!(
        "reactors of {} at {} (before {:?}, limit {}) via list_reactions",
        args.chat, args.timestamp, args.before, args.limit
    );
    println!(
        "{:#?}",
        client
            .reactors(&args.chat, args.timestamp, args.before, args.limit)
            .await
            .map_err(fail)?
    );
    Ok(())
}

/// What `create-private <guid> [flags]` asks for.
#[derive(Debug, PartialEq, Eq)]
enum CreatePrivate<'a> {
    /// The confirmation flag is present: send the request.
    Send(&'a str),
    /// No flag: print the target and exit without a request.
    Refuse(&'a str),
}

/// `<guid>` alone refuses, `<guid> --confirm-create` sends; a blank guid or any other flag is a
/// usage error (`None`).
fn create_private_args<'a>(guid: &'a str, flags: &[&str]) -> Option<CreatePrivate<'a>> {
    if guid.trim().is_empty() {
        return None;
    }
    match flags {
        [] => Some(CreatePrivate::Refuse(guid)),
        ["--confirm-create"] => Some(CreatePrivate::Send(guid)),
        _ => None,
    }
}

fn refusal(guid: &str) -> String {
    format!(
        "create-private would open (creating it when missing) the private chat with {guid} on the real account; nothing was sent. Repeat with --confirm-create to send it"
    )
}

/// A creation that did not succeed, with the exit code that tells a safe repeat (1) from a
/// possible duplicate (3).
fn creation_failure(error: ManageError) -> Failure {
    match error {
        ManageError::NotSent(e) => (1, format!("not sent, nothing was created: {e}")),
        ManageError::Rejected(e) => (
            1,
            format!("rejected by the server, nothing was created: {e}"),
        ),
        ManageError::Unknown(e) => (
            3,
            format!(
                "outcome unknown, the chat may exist: check the web client before repeating: {e}"
            ),
        ),
    }
}

/// One `create_private_chat` (registry mutation with CSRF, sent once, never retried). Reached
/// only with `--confirm-create`. The raw reply is visible at `BT_LOG=yandex_proto::registry=trace`.
async fn create_private(guid: &str) -> Result<(), Failure> {
    creation_guard(write_only()?.as_deref())?;
    let (client, _events) =
        Client::start(&load_session()?, ClientConfig::production()).map_err(fail)?;
    eprintln!("creating the private chat with {guid}");
    match client
        .create_private_chat(guid)
        .await
        .map_err(creation_failure)?
    {
        Some(chat) => println!("created: chat {chat}"),
        None => println!("created: the reply names no chat id (look in the chat list)"),
    }
    Ok(())
}

/// What `create-chat <title> <guid>... [--confirm-create]` asks for.
#[derive(Debug, PartialEq, Eq)]
struct GroupPlan<'a> {
    title: &'a str,
    users: Vec<&'a str>,
    /// `--confirm-create` was given: send the request.
    confirmed: bool,
}

/// A blank or over-long title, no participant, a blank, repeated or flag-looking guid, or a
/// repeated `--confirm-create` is a usage error (`None`). `--confirm-create` may stand anywhere
/// among the guids.
fn create_chat_args<'a>(title: &'a str, rest: &[&'a str]) -> Option<GroupPlan<'a>> {
    if title.trim().is_empty() || title.chars().count() > MAX_CHAT_TITLE {
        return None;
    }
    let mut users: Vec<&str> = Vec::new();
    let mut confirmed = false;
    for arg in rest {
        if *arg == "--confirm-create" {
            if confirmed {
                return None;
            }
            confirmed = true;
        } else if arg.trim().is_empty() || arg.starts_with('-') || users.contains(arg) {
            return None;
        } else {
            users.push(*arg);
        }
    }
    (!users.is_empty()).then_some(GroupPlan {
        title,
        users,
        confirmed,
    })
}

/// The refusal printed without `--confirm-create`: everything that would be sent.
fn group_refusal(plan: &GroupPlan<'_>) -> String {
    let mut text = format!(
        "create-chat would create a group chat on the real account; nothing was sent.\n  title: {:?}\n  description: \"\"\n  channel: false\n  is_public: true\n  participants ({}):",
        plan.title,
        plan.users.len()
    );
    for user in &plan.users {
        text.push_str("\n    ");
        text.push_str(user);
    }
    text.push_str("\nRepeat with --confirm-create to send it");
    text
}

/// One `create_chat` (registry mutation with CSRF, sent once, never retried). Reached only with
/// `--confirm-create`. The raw reply is visible at `BT_LOG=yandex_proto::registry=trace`.
async fn create_chat(plan: GroupPlan<'_>) -> Result<(), Failure> {
    creation_guard(write_only()?.as_deref())?;
    let (client, _events) =
        Client::start(&load_session()?, ClientConfig::production()).map_err(fail)?;
    eprintln!(
        "creating the group chat with {} participant(s)",
        plan.users.len()
    );
    let users: Vec<String> = plan.users.iter().map(|user| (*user).to_owned()).collect();
    let created = client
        .create_chat(plan.title, &users)
        .await
        .map_err(creation_failure)?;
    match created.chat {
        Some(chat) => println!("created: chat {chat}"),
        None => println!("created: the reply names no chat id (look in the chat list)"),
    }
    if !created.refused.is_empty() {
        println!(
            "not added by the server ({}): {}",
            created.refused.len(),
            created.refused.join(", ")
        );
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UrlVariant {
    /// `MediaTarget::Original`: what the app's Download sends.
    Original,
    /// `MediaTarget::Playback`: the web client's `voicePlayUrl`.
    Plain,
    /// `MediaTarget::VideoStream`: the web client's `videoStreamsUrl`.
    Stream,
}

impl UrlVariant {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "original" => Some(Self::Original),
            "plain" => Some(Self::Plain),
            "stream" => Some(Self::Stream),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Plain => "plain",
            Self::Stream => "stream",
        }
    }

    fn target(self, file_id: String) -> MediaTarget {
        match self {
            Self::Original => MediaTarget::Original { file_id },
            Self::Plain => MediaTarget::Playback { file_id },
            Self::Stream => MediaTarget::VideoStream { file_id },
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DownloadFlags<'a> {
    /// 1-based position inside a gallery.
    item: Option<usize>,
    to: Option<&'a str>,
    url: Option<UrlVariant>,
}

fn download_flags<'a>(mut flags: &[&'a str]) -> Option<DownloadFlags<'a>> {
    let mut parsed = DownloadFlags::default();
    while let [name, value, rest @ ..] = flags {
        match *name {
            "--item" => parsed.item = Some(value.parse().ok().filter(|n| *n >= 1)?),
            "--to" => parsed.to = Some(value),
            "--url" => parsed.url = Some(UrlVariant::parse(value)?),
            _ => return None,
        }
        flags = rest;
    }
    flags.is_empty().then_some(parsed)
}

/// The file a download takes out of a message body: its position (from 1) among `count`.
#[derive(Debug, PartialEq, Eq)]
struct Chosen<'a> {
    file: &'a WireFile,
    position: usize,
    count: usize,
}

fn select_file(body: &WireBody, item: Option<usize>) -> Result<Chosen<'_>, String> {
    let single = |file| match item {
        None | Some(1) => Ok(Chosen {
            file,
            position: 1,
            count: 1,
        }),
        Some(n) => Err(format!("--item {n}: the message holds one file")),
    };
    match body {
        WireBody::File(file) | WireBody::Voice { file, .. } => single(file),
        WireBody::Image(image) => single(&image.file),
        WireBody::Gallery { items, .. } => {
            let position = item.unwrap_or(1);
            let image = position.checked_sub(1).and_then(|index| items.get(index));
            let image = image.ok_or_else(|| {
                format!(
                    "--item {position}: the gallery holds {} images",
                    items.len()
                )
            })?;
            Ok(Chosen {
                file: &image.file,
                position,
                count: items.len(),
            })
        }
        _ => Err("the message holds no file or image".to_owned()),
    }
}

/// The local name for a downloaded file. The server's name is untrusted: only its last path
/// component survives, without control characters, colons and leading dots; an unusable name
/// falls back to the last segment of the file id (which `MediaTarget::url` has validated).
fn local_name(file: &WireFile) -> String {
    const MAX_BYTES: usize = 200;
    let last = file
        .name
        .as_deref()
        .and_then(|name| name.rsplit(['/', '\\']).next())
        .unwrap_or("");
    let cleaned: String = last
        .chars()
        .map(|c| if c.is_control() || c == ':' { '_' } else { c })
        .collect();
    let mut name = cleaned.trim().trim_start_matches('.');
    while name.len() > MAX_BYTES {
        let mut cut = MAX_BYTES;
        while !name.is_char_boundary(cut) {
            cut -= 1;
        }
        name = &name[..cut];
    }
    if name.is_empty() {
        file.id.rsplit('/').next().unwrap_or("file").to_owned()
    } else {
        name.to_owned()
    }
}

/// Host and path of a URL: no scheme, no query, no fragment.
fn host_and_path(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['?', '#']).next().unwrap_or(rest)
}

/// Creates `name` in `dir`; an existing entry of that name is an error and is left untouched.
fn create_destination(dir: &Path, name: &str) -> Result<File, String> {
    let path = dir.join(name);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| match e.kind() {
            ErrorKind::AlreadyExists => format!("refusing to overwrite {}", path.display()),
            kind => format!("cannot create {}: {kind}", path.display()),
        })
}

fn error_class(error: &ProtoError) -> &'static str {
    match error {
        ProtoError::SessionExpired => "session-expired",
        ProtoError::Network(_) => "network",
        ProtoError::Protocol(_) => "protocol",
        ProtoError::NotFound => "not-found",
        ProtoError::Storage(_) => "storage",
        ProtoError::Commit { .. } => "commit",
    }
}

fn or_dash(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_owned(), |v| v.to_string())
}

async fn download(chat: &str, timestamp: i64, flags: DownloadFlags<'_>) -> Result<(), Failure> {
    if let Some(variant) = flags.url {
        return sample_download(chat, timestamp, &flags, variant).await;
    }
    let dir = Path::new(flags.to.unwrap_or("."));
    if !dir.is_dir() {
        return Err((2, format!("--to {}: not a directory", dir.display())));
    }
    let (client, _events) = connect(&load_session()?, ClientConfig::production()).await?;
    let message = find_message(&client, chat, timestamp).await?;
    let chosen = select_file(&message.body, flags.item)
        .map_err(|e| (1, format!("message {timestamp}: {e}")))?;
    let target = MediaTarget::Original {
        file_id: chosen.file.id.clone(),
    };
    let url = target.url(&MediaConfig::production()).map_err(fail)?;
    let destination = dir.join(local_name(chosen.file));
    let mut out = create_destination(dir, &local_name(chosen.file)).map_err(|e| (1, e))?;
    let announced = Mutex::new(None);
    let progress = |_received: u64, total: Option<u64>| *announced.lock().unwrap() = total;
    let result = client.download_media(&target, &mut out, &progress).await;
    drop(out);
    let received = result.map_err(|e| {
        // A partial file under the final name would look like a finished download.
        let _ = fs::remove_file(&destination);
        let class = error_class(&e);
        (1, format!("download failed ({class}): {}", fail(e).1))
    })?;
    let content_length = *announced.lock().unwrap();
    println!(
        "message {timestamp}: item {} of {}",
        chosen.position, chosen.count
    );
    println!("saved: {}", destination.display());
    println!("received: {received} bytes");
    println!(
        "announced size (FileInfo.Size): {}",
        or_dash(chosen.file.size)
    );
    println!("content-length: {}", or_dash(content_length));
    let verdict = match chosen.file.size {
        Some(size) if size == received => "matches",
        Some(_) => "DIFFERS",
        None => "unknown",
    };
    println!("size check: {verdict}");
    // `download_media` returns only on a 2xx answer; the redirect target is in the debug log
    // (`BT_LOG=yandex_proto=debug`, field `landed`).
    println!("status: 2xx");
    println!("target: {}", host_and_path(&url));
    Ok(())
}

/// The sample directory of the research: it holds message content, so it must exist already
/// and be reachable by its owner only.
fn check_sample_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let meta =
        fs::metadata(dir).map_err(|e| format!("--to: cannot read the directory: {}", e.kind()))?;
    if !meta.is_dir() {
        return Err("--to: not a directory".to_owned());
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err("--to: the sample directory must have mode 0700".to_owned());
    }
    Ok(())
}

/// The file name of a sample: timestamp, position, URL variant and, when the server's name has
/// a short alphanumeric extension, that extension (a player may look at it). Nothing else of the
/// server's name is kept.
fn sample_name(timestamp: i64, chosen: &Chosen<'_>, variant: UrlVariant) -> String {
    let name = local_name(chosen.file);
    let extension = Path::new(&name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| (1..=8).contains(&e.len()) && e.bytes().all(|b| b.is_ascii_alphanumeric()));
    let stem = format!("{timestamp}-{}-{}", chosen.position, variant.label());
    match extension {
        Some(extension) => format!("{stem}.{extension}"),
        None => stem,
    }
}

/// SHA-256 (FIPS 180-4), kept here so the probe needs no new dependency.
struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    filled: usize,
    length: u64,
}

impl Sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            block: [0; 64],
            filled: 0,
            length: 0,
        }
    }

    fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (word, bytes) in w.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
        for (k, word) in Self::K.iter().zip(w) {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(*k)
                .wrapping_add(word);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (word, add) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *word = word.wrapping_add(add);
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.length += data.len() as u64;
        while !data.is_empty() {
            let take = data.len().min(64 - self.filled);
            self.block[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == 64 {
                Self::compress(&mut self.state, &self.block);
                self.filled = 0;
            }
        }
    }

    /// The digest as 64 lower-case hex digits.
    fn finish(mut self) -> String {
        let bits = self.length * 8;
        self.update(&[0x80]);
        while self.filled != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        self.state
            .iter()
            .map(|word| format!("{word:08x}"))
            .collect()
    }
}

/// Writes through to `inner` and hashes exactly what `inner` accepted.
struct Hashing<W> {
    inner: W,
    hash: Sha256,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hash.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// `download --url`: one request for the chosen URL variant into the sample directory,
/// reported as the research needs it and nothing more (no path, no name, no query, no body).
async fn sample_download(
    chat: &str,
    timestamp: i64,
    flags: &DownloadFlags<'_>,
    variant: UrlVariant,
) -> Result<(), Failure> {
    let dir = Path::new(flags.to.ok_or_else(|| {
        (
            2,
            "--url needs --to <sample directory> (existing, mode 0700)".to_owned(),
        )
    })?);
    check_sample_dir(dir).map_err(|e| (2, e))?;
    let session = load_session()?;
    let config = ClientConfig::production();
    let media = MediaClient::new(config.media.clone(), &config.http, session.cookies.clone())
        .map_err(fail)?;
    let (client, _events) = connect(&session, config).await?;
    let message = find_message(&client, chat, timestamp).await?;
    let chosen = select_file(&message.body, flags.item)
        .map_err(|e| (1, format!("message {timestamp}: {e}")))?;
    let target = variant.target(chosen.file.id.clone());
    let name = sample_name(timestamp, &chosen, variant);
    let mut out = Hashing {
        inner: create_destination(dir, &name).map_err(|e| (1, e))?,
        hash: Sha256::new(),
    };
    let result = media.download_exchange(&target, &mut out).await;
    let Hashing { inner, hash } = out;
    drop(inner);
    let exchange = result.map_err(|e| {
        // A partial file under the final name would look like a finished download.
        let _ = fs::remove_file(dir.join(&name));
        let class = error_class(&e);
        (1, format!("download failed ({class}): {}", fail(e).1))
    })?;
    let show = |value: Option<String>| value.unwrap_or_else(|| "-".to_owned());
    println!("status: {}", exchange.status);
    println!("landed: {}", exchange.landed_host);
    println!("content-type: {}", show(exchange.content_type));
    println!("content-length: {}", or_dash(exchange.content_length));
    println!("accept-ranges: {}", show(exchange.accept_ranges));
    println!(
        "content-disposition: {}",
        if exchange.has_content_disposition {
            "present"
        } else {
            "absent"
        }
    );
    println!("received: {} bytes", exchange.received);
    if (200..300).contains(&exchange.status) {
        println!("sha256: {}", hash.finish());
        return Ok(());
    }
    println!("sha256: -");
    let _ = fs::remove_file(dir.join(&name));
    Err(match exchange.status {
        401 => fail(ProtoError::SessionExpired),
        status => (1, format!("status {status}: no body was read")),
    })
}

async fn listen(watch: Option<&str>) -> Result<(), Failure> {
    let (client, mut events) = connect(&load_session()?, ClientConfig::production()).await?;
    if let Some(chat) = watch {
        client.watch_chats(vec![chat.to_owned()]);
    }
    while let Some(event) = events.recv().await {
        match event {
            XivaEvent::SessionExpired => return Err(fail(ProtoError::SessionExpired)),
            XivaEvent::Push(push) => println!("{push:?}"),
            other => println!("{other:?}"),
        }
    }
    Ok(())
}

fn describe(message: &WireMessage) -> String {
    let author = message
        .from
        .as_ref()
        .and_then(|f| f.display_name.as_deref())
        .unwrap_or("?");
    let body = match &message.body {
        WireBody::Text(text) => text.clone(),
        other => format!("[{other:?}]"),
    };
    let flags = format!(
        "{}{}",
        if message.edited { " (edited)" } else { "" },
        if message.important {
            " (important)"
        } else {
            ""
        }
    );
    let reply = match &message.relation {
        Relation::Reply {
            quote, original, ..
        } => {
            let target = original
                .as_ref()
                .and_then(|o| o.timestamp)
                .map_or_else(|| "?".to_owned(), |t| t.to_string());
            format!(
                " (reply to {target}, quote {:?})",
                quote.as_deref().unwrap_or("")
            )
        }
        _ => String::new(),
    };
    format!(
        "{}\t{author}\t{body}{flags}{reply}{}",
        message.timestamp,
        reactions_note(message)
    )
}

/// The reaction state of a history row, as the server sent it: counts `code×count`, the version
/// and the recent reactors' codes. Empty when the row carried none of the three siblings.
fn reactions_note(message: &WireMessage) -> String {
    let Some(reactions) = &message.reactions else {
        return String::new();
    };
    let counts: Vec<String> = reactions
        .counts
        .iter()
        .map(|c| format!("{}x{}", c.code, c.count))
        .collect();
    let recent: Vec<String> = reactions
        .recent
        .iter()
        .map(|r| format!("{}:{}", r.user.as_deref().unwrap_or("?"), r.code))
        .collect();
    let version = reactions
        .version
        .map_or_else(|| "-".to_owned(), |v| v.to_string());
    format!(
        " (reactions [{}], version {version}, recent [{}])",
        counts.join(" "),
        recent.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use yandex_proto::wire::{WireChatKind, WireForwarded, WireRef};

    const GUID: &str = "c9a71201-70b5-992e-61f2-b27e2d3fc8fb";

    fn message(relation: Relation) -> WireMessage {
        WireMessage {
            chat: "c".into(),
            timestamp: 20,
            seq_no: None,
            version: None,
            payload_id: None,
            from: None,
            body: WireBody::Text("mine".into()),
            edited: false,
            important: false,
            relation,
            reactions: None,
            raw: serde_json::Value::Null,
        }
    }

    fn original(timestamp: Option<i64>, body: WireBody) -> WireForwarded {
        WireForwarded {
            chat: None,
            timestamp,
            from: None,
            body,
        }
    }

    #[test]
    fn an_edit_repeats_the_stored_quote_not_the_current_text_of_the_original() {
        // The original was edited after the reply: the quote still shows the old text.
        let edited_since = Relation::Reply {
            quote: Some("old text".into()),
            reference: None,
            original: Some(original(Some(10), WireBody::Text("new text".into()))),
        };
        assert_eq!(
            stored_reply(&message(edited_since)),
            Ok(Some(ReplyRef {
                timestamp: 10,
                quote: "old text".into()
            }))
        );
        // The original was deleted: no original body, the quote and target still come from the reply.
        let deleted = Relation::Reply {
            quote: Some("old text".into()),
            reference: None,
            original: Some(original(Some(10), WireBody::Empty)),
        };
        assert_eq!(
            stored_reply(&message(deleted)),
            Ok(Some(ReplyRef {
                timestamp: 10,
                quote: "old text".into()
            }))
        );
        // A reference on the message wins over the snapshot for the target.
        let referenced = Relation::Reply {
            quote: Some("q".into()),
            reference: Some(WireRef {
                chat: None,
                timestamp: Some(7),
            }),
            original: Some(original(Some(10), WireBody::Empty)),
        };
        assert_eq!(
            stored_reply(&message(referenced)),
            Ok(Some(ReplyRef {
                timestamp: 7,
                quote: "q".into()
            }))
        );
    }

    #[test]
    fn stored_reply_falls_back_to_the_snapshot_text_and_refuses_what_it_cannot_keep() {
        let no_quote = Relation::Reply {
            quote: None,
            reference: None,
            original: Some(original(Some(10), WireBody::Text("orig".into()))),
        };
        assert_eq!(
            stored_reply(&message(no_quote)),
            Ok(Some(ReplyRef {
                timestamp: 10,
                quote: "orig".into()
            }))
        );
        assert_eq!(stored_reply(&message(Relation::None)), Ok(None));
        let unknown_target = Relation::Reply {
            quote: Some("q".into()),
            reference: None,
            original: None,
        };
        assert!(
            stored_reply(&message(unknown_target))
                .unwrap_err()
                .contains("--reply")
        );
        let nothing_to_quote = Relation::Reply {
            quote: None,
            reference: None,
            original: Some(original(Some(10), WireBody::Empty)),
        };
        assert!(stored_reply(&message(nothing_to_quote)).is_err());
        assert!(
            stored_reply(&message(Relation::Forward { originals: vec![] }))
                .unwrap_err()
                .contains("forward")
        );
    }

    #[test]
    fn self_commands_act_on_the_self_chat_and_refuse_every_other_chat() {
        assert_eq!(
            resolve_self_chat(None, GUID).unwrap(),
            format!("{GUID}_{GUID}")
        );
        assert_eq!(
            resolve_self_chat(Some(&format!("{GUID}_{GUID}")), GUID).unwrap(),
            format!("{GUID}_{GUID}")
        );
        for other in [
            "0/0/some-group",
            "dc8fe606-16f9-97d9-4faf-3acb552a7764_c9a71201-70b5-992e-61f2-b27e2d3fc8fb",
            GUID,
            "",
        ] {
            let error = resolve_self_chat(Some(other), GUID).unwrap_err();
            assert!(error.contains("self chat only"), "{other}: {error}");
        }
    }

    #[test]
    fn send_flags_parse_in_any_order_and_reject_the_rest() {
        assert_eq!(send_flags(&[]), Some(SendFlags::default()));
        assert_eq!(
            send_flags(&["--reply", "17", "--payload", "p-1"]),
            Some(SendFlags {
                payload: Some("p-1"),
                reply: Some(17),
                chat: None
            })
        );
        assert_eq!(
            send_flags(&["--chat", "c"]),
            Some(SendFlags {
                chat: Some("c"),
                ..SendFlags::default()
            })
        );
        for bad in [
            &["--reply", "x"][..],
            &["--payload"],
            &["--unknown", "1"],
            &["--reply", "1", "stray"],
        ] {
            assert_eq!(send_flags(bad), None, "{bad:?}");
        }
        assert_eq!(
            edit_flags(&["--reply", "17"]),
            Some(EditFlags {
                send: SendFlags {
                    reply: Some(17),
                    ..SendFlags::default()
                },
                important: false,
            })
        );
        assert_eq!(edit_flags(&["--payload", "p"]), None);
        assert_eq!(edit_flags(&[]), Some(EditFlags::default()));
        assert_eq!(
            edit_flags(&["--important"]),
            Some(EditFlags {
                important: true,
                ..EditFlags::default()
            })
        );
        assert_eq!(
            edit_flags(&["--chat", "c", "--important", "--reply", "3"]),
            Some(EditFlags {
                send: SendFlags {
                    reply: Some(3),
                    chat: Some("c"),
                    payload: None
                },
                important: true,
            })
        );
        for bad in [
            &["--important", "x"][..],
            &["--reply", "--important"],
            &["--important", "--payload", "p"],
        ] {
            assert_eq!(edit_flags(bad), None, "{bad:?}");
        }
        assert_eq!(chat_flag(&[]), Some(None));
        assert_eq!(chat_flag(&["--chat", "c"]), Some(Some("c")));
        assert_eq!(chat_flag(&["c"]), None);
    }

    #[test]
    fn react_self_takes_a_positive_code_or_remove() {
        assert_eq!(
            reaction_change("100101"),
            Some(ReactionChange::Set(100_101))
        );
        assert_eq!(reaction_change("remove"), Some(ReactionChange::Remove));
        for bad in [
            "",
            "0",
            "-5",
            "x",
            "1.5",
            "Remove",
            "remove ",
            "100101 ",
            "99999999999999999999",
        ] {
            assert_eq!(reaction_change(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn react_self_refuses_a_chat_other_than_the_self_chat() {
        // The command goes through `write_self`, so `resolve_self_chat` is what guards the chat.
        assert_eq!(chat_flag(&["--chat", "c"]), Some(Some("c")));
        let error = resolve_self_chat(Some("0/0/some-group"), GUID).unwrap_err();
        assert!(error.contains("self chat only"), "{error}");
        assert_eq!(
            resolve_self_chat(None, GUID).unwrap(),
            format!("{GUID}_{GUID}")
        );
    }

    #[test]
    fn a_history_row_lists_its_reactions_only_when_it_has_them() {
        use yandex_proto::wire::{WireReactionCount, WireReactions, WireReactor};
        let mut row = message(Relation::None);
        assert_eq!(describe(&row), "20\t?\tmine");
        row.reactions = Some(WireReactions {
            counts: vec![WireReactionCount {
                code: 100_101,
                count: 2,
            }],
            version: Some(3),
            recent: vec![WireReactor {
                user: Some("u".into()),
                code: 100_101,
                timestamp: None,
            }],
        });
        assert_eq!(
            describe(&row),
            "20\t?\tmine (reactions [100101x2], version 3, recent [u:100101])"
        );
        row.reactions = Some(WireReactions {
            version: Some(4),
            ..WireReactions::default()
        });
        assert_eq!(
            describe(&row),
            "20\t?\tmine (reactions [], version 4, recent [])"
        );
    }

    #[test]
    fn a_history_row_is_marked_important_only_when_the_flag_is_set() {
        let mut row = message(Relation::None);
        assert_eq!(describe(&row), "20\t?\tmine");
        row.important = true;
        assert_eq!(describe(&row), "20\t?\tmine (important)");
        row.edited = true;
        assert_eq!(describe(&row), "20\t?\tmine (edited) (important)");
    }

    fn chat(pinned: Option<i64>, pin_action_ts: Option<i64>) -> WireChat {
        WireChat {
            id: "c1".into(),
            kind: WireChatKind::Group,
            title: Some("Team".into()),
            avatar_id: None,
            partner_guid: None,
            last_activity: Some(500),
            unread: 2,
            muted: false,
            pinned,
            pin_action_ts,
            last_message: None,
            raw: serde_json::Value::Null,
        }
    }

    #[test]
    fn a_chat_line_prints_the_pinned_timestamp_only_when_the_chat_has_a_pin() {
        assert_eq!(
            chat_line(&chat(None, None)),
            "c1\tGroup\tTeam\tunread=2\tts=500\t"
        );
        assert_eq!(
            chat_line(&chat(Some(123), Some(900))),
            "c1\tGroup\tTeam\tunread=2 pinned=123\tts=500\t"
        );
        // An unpin snapshot keeps its action time but is no pin.
        assert_eq!(
            chat_line(&chat(None, Some(900))),
            "c1\tGroup\tTeam\tunread=2\tts=500\t"
        );
        let mut muted = chat(Some(7), None);
        muted.muted = true;
        assert_eq!(
            chat_line(&muted),
            "c1\tGroup\tTeam\tunread=2 muted pinned=7\tts=500\t"
        );
    }

    #[test]
    fn create_private_refuses_without_the_flag_and_names_the_target() {
        assert_eq!(
            create_private_args(GUID, &[]),
            Some(CreatePrivate::Refuse(GUID))
        );
        assert_eq!(
            create_private_args(GUID, &["--confirm-create"]),
            Some(CreatePrivate::Send(GUID))
        );
        for bad in [
            &["--confirm"][..],
            &["--confirm-create", "--confirm-create"],
            &["--confirm-create", "x"],
            &["yes"],
        ] {
            assert_eq!(create_private_args(GUID, bad), None, "{bad:?}");
        }
        assert_eq!(create_private_args("  ", &["--confirm-create"]), None);
        assert_eq!(create_private_args("", &[]), None);
        let message = refusal(GUID);
        assert!(
            message.contains(GUID)
                && message.contains("nothing was sent")
                && message.contains("--confirm-create"),
            "{message}"
        );
    }

    #[test]
    fn create_chat_refuses_without_the_flag_and_prints_what_it_would_send() {
        let plan = create_chat_args("Team A", &[GUID, "g-2"]).unwrap();
        assert_eq!(
            plan,
            GroupPlan {
                title: "Team A",
                users: vec![GUID, "g-2"],
                confirmed: false
            }
        );
        let message = group_refusal(&plan);
        for needle in [
            "\"Team A\"",
            GUID,
            "g-2",
            "is_public: true",
            "channel: false",
            "nothing was sent",
            "--confirm-create",
        ] {
            assert!(message.contains(needle), "{needle}: {message}");
        }
        // The flag may stand anywhere among the guids.
        for rest in [
            &[GUID, "g-2", "--confirm-create"][..],
            &["--confirm-create", GUID, "g-2"],
            &[GUID, "--confirm-create", "g-2"],
        ] {
            assert_eq!(
                create_chat_args("Team A", rest),
                Some(GroupPlan {
                    title: "Team A",
                    users: vec![GUID, "g-2"],
                    confirmed: true
                })
            );
        }
    }

    #[test]
    fn a_write_limit_allows_only_the_chat_it_names() {
        let own = format!("{GUID}_{GUID}");
        assert_eq!(chat_write_guard(None, &own), Ok(()));
        assert_eq!(chat_write_guard(Some(&own), &own), Ok(()));
        let error = chat_write_guard(Some(&own), "0/0/some-group").unwrap_err();
        assert!(
            error.contains("BT_WRITE_ONLY_CHAT")
                && error.contains("0/0/some-group")
                && error.contains("nothing was sent"),
            "{error}"
        );
    }

    #[test]
    fn the_limit_is_read_like_the_desktop_app_reads_it() {
        assert_eq!(parse_write_only(None), Ok(None));
        assert_eq!(
            parse_write_only(Some("0/0/c".into())),
            Ok(Some("0/0/c".into()))
        );
        for blank in ["", "  ", "\t"] {
            assert!(
                parse_write_only(Some(blank.into()))
                    .unwrap_err()
                    .contains("set but empty"),
                "{blank:?}"
            );
        }
    }

    #[test]
    fn self_writes_are_refused_before_connecting_when_the_limit_names_another_chat() {
        // `send-self`, `edit-self`, `delete-self`, `typing-self` and `react-self` all take their chat
        // from `guarded_self_chat` before the connection is made.
        let own = format!("{GUID}_{GUID}");
        assert_eq!(guarded_self_chat(None, GUID, None), Ok(own.clone()));
        assert_eq!(guarded_self_chat(None, GUID, Some(&own)), Ok(own.clone()));
        let (code, message) = guarded_self_chat(None, GUID, Some("0/0/some-group")).unwrap_err();
        assert_eq!(code, 2);
        assert!(
            message.contains("BT_WRITE_ONLY_CHAT") && message.contains("nothing was sent"),
            "{message}"
        );
        // `--chat` naming another chat stays an error of its own.
        let (code, message) =
            guarded_self_chat(Some("0/0/some-group"), GUID, Some("0/0/some-group")).unwrap_err();
        assert_eq!((code, message.contains("self chat only")), (2, true));
    }

    #[test]
    fn creation_is_refused_whenever_a_limit_is_set() {
        assert_eq!(creation_guard(None), Ok(()));
        for allowed in [format!("{GUID}_{GUID}"), "0/0/some-group".to_owned()] {
            let (code, message) = creation_guard(Some(&allowed)).unwrap_err();
            assert_eq!(code, 2, "{allowed}");
            assert!(
                message.contains("BT_WRITE_ONLY_CHAT") && message.contains("nothing was sent"),
                "{message}"
            );
        }
    }

    #[test]
    fn create_chat_rejects_what_the_web_client_would_not_send() {
        let long = "x".repeat(MAX_CHAT_TITLE + 1);
        assert!(create_chat_args(&"x".repeat(MAX_CHAT_TITLE), &[GUID]).is_some());
        for (title, rest) in [
            ("", &[GUID][..]),
            ("  ", &[GUID]),
            (long.as_str(), &[GUID]),
            ("T", &[]),
            ("T", &["--confirm-create"]),
            ("T", &[GUID, GUID]),
            ("T", &[GUID, " "]),
            ("T", &[GUID, "--confirm"]),
            ("T", &[GUID, "--confirm-create", "--confirm-create"]),
        ] {
            assert_eq!(create_chat_args(title, rest), None, "{title:?} {rest:?}");
        }
    }

    #[test]
    fn only_an_unknown_outcome_warns_about_a_duplicate() {
        let e = || ProtoError::Network("boom".into());
        assert_eq!(creation_failure(ManageError::NotSent(e())).0, 1);
        assert_eq!(creation_failure(ManageError::Rejected(e())).0, 1);
        let (code, message) = creation_failure(ManageError::Unknown(e()));
        assert_eq!(code, 3);
        assert!(message.contains("may exist"), "{message}");
    }
}

#[cfg(test)]
mod download_tests {
    use super::*;
    use yandex_proto::wire::WireImage;

    fn file(id: &str, name: Option<&str>) -> WireFile {
        WireFile {
            id: id.into(),
            name: name.map(Into::into),
            size: Some(5),
        }
    }

    fn image(id: &str) -> WireImage {
        WireImage {
            file: file(id, None),
            width: 1,
            height: 1,
            animated: false,
        }
    }

    #[test]
    fn download_flags_parse_in_any_order_and_reject_the_rest() {
        assert_eq!(download_flags(&[]), Some(DownloadFlags::default()));
        assert_eq!(
            download_flags(&["--to", "/tmp/x", "--item", "3"]),
            Some(DownloadFlags {
                item: Some(3),
                to: Some("/tmp/x"),
                url: None
            })
        );
        assert_eq!(
            download_flags(&["--item", "1"]),
            Some(DownloadFlags {
                item: Some(1),
                to: None,
                url: None
            })
        );
        for bad in [
            &["--item", "0"][..],
            &["--item", "x"],
            &["--item", "-1"],
            &["--item"],
            &["--to"],
            &["--unknown", "1"],
            &["--item", "1", "stray"],
        ] {
            assert_eq!(download_flags(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn select_file_takes_the_file_or_the_nth_image_and_refuses_the_rest() {
        let one = file("disk/a", Some("a.bin"));
        let body = WireBody::File(one.clone());
        assert_eq!(
            select_file(&body, None),
            Ok(Chosen {
                file: &one,
                position: 1,
                count: 1
            })
        );
        assert!(
            select_file(&body, Some(2))
                .unwrap_err()
                .contains("one file")
        );
        let single = WireBody::Image(image("disk/i"));
        assert_eq!(select_file(&single, Some(1)).unwrap().file.id, "disk/i");
        assert!(select_file(&single, Some(2)).is_err());
        let gallery = WireBody::Gallery {
            items: vec![image("disk/1"), image("disk/2"), image("disk/3")],
            text: None,
        };
        let third = select_file(&gallery, Some(3)).unwrap();
        assert_eq!(
            (third.file.id.as_str(), third.position, third.count),
            ("disk/3", 3, 3)
        );
        assert_eq!(select_file(&gallery, None).unwrap().file.id, "disk/1");
        assert!(
            select_file(&gallery, Some(4))
                .unwrap_err()
                .contains("holds 3 images")
        );
        assert!(
            select_file(
                &WireBody::Gallery {
                    items: vec![],
                    text: None
                },
                None
            )
            .is_err()
        );
        for other in [
            WireBody::Text("t".into()),
            WireBody::Deleted,
            WireBody::Empty,
        ] {
            assert!(
                select_file(&other, None)
                    .unwrap_err()
                    .contains("no file or image"),
                "{other:?}"
            );
        }
    }

    #[test]
    fn local_names_cannot_leave_the_directory() {
        let name = |raw: Option<&str>| local_name(&file("disk/0a1b-2c", raw));
        assert_eq!(name(Some("report.pdf")), "report.pdf");
        assert_eq!(name(Some("../../.ssh/x")), "x");
        assert_eq!(name(Some("a/b\\c.txt")), "c.txt");
        assert_eq!(name(Some("a\nb:c")), "a_b_c");
        assert_eq!(name(Some("..hidden")), "hidden");
        for unusable in [
            None,
            Some(""),
            Some("."),
            Some(".."),
            Some("dir/"),
            Some("  "),
        ] {
            assert_eq!(name(unusable), "0a1b-2c", "{unusable:?}");
        }
        let long = "я".repeat(150) + ".txt";
        let cut = name(Some(&long));
        assert!(
            cut.len() <= 200 && cut.chars().all(|c| c == 'я'),
            "{} bytes",
            cut.len()
        );
    }

    #[test]
    fn the_printed_target_has_host_and_path_only() {
        let url = MediaTarget::Original {
            file_id: "disk/0a1b-2c".into(),
        }
        .url(&MediaConfig::production())
        .unwrap();
        assert_eq!(
            host_and_path(&url),
            "files.messenger.yandex.ru/file_shortterm/disk/0a1b-2c"
        );
        assert_eq!(host_and_path("https://h/p?token=secret#frag"), "h/p");
        assert_eq!(host_and_path("h/p"), "h/p");
    }

    #[test]
    fn an_existing_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.bin"), b"keep").unwrap();
        let error = create_destination(dir.path(), "a.bin").unwrap_err();
        assert!(error.contains("refusing to overwrite"), "{error}");
        assert_eq!(std::fs::read(dir.path().join("a.bin")).unwrap(), b"keep");
        assert!(create_destination(dir.path(), "b.bin").is_ok());
        assert!(create_destination(dir.path(), "b.bin").is_err());
    }

    #[test]
    fn every_error_has_a_class() {
        assert_eq!(error_class(&ProtoError::Storage("x".into())), "storage");
        assert_eq!(error_class(&ProtoError::NotFound), "not-found");
        assert_eq!(error_class(&ProtoError::SessionExpired), "session-expired");
        assert_eq!(error_class(&ProtoError::Network("n".into())), "network");
        assert_eq!(error_class(&ProtoError::Protocol("p".into())), "protocol");
    }

    #[test]
    fn a_refusal_fails_without_the_servers_reason() {
        // The live `list_reactions` refusal of a channel: `ErrorInfo.Message`, no `Details`.
        let refused = yandex_proto::wire::check_status(&serde_json::json!({
            "ErrorInfo": { "Message": "server-secret-reason", "WorkerId": 1 },
            "Status": 4
        }))
        .unwrap_err();
        assert_eq!(
            fail(refused.clone()),
            (
                1,
                "unexpected server response: status 4 ENTITY_NOT_FOUND".to_owned()
            )
        );
        // The download failure line embeds the same text.
        assert!(!fail(refused).1.contains("server-secret-reason"));
        assert_eq!(
            fail(ProtoError::Protocol("search: no data".into())).1,
            "unexpected server response: search: no data"
        );
    }

    #[test]
    fn the_url_flag_names_one_of_three_variants() {
        for (name, variant) in [
            ("original", UrlVariant::Original),
            ("plain", UrlVariant::Plain),
            ("stream", UrlVariant::Stream),
        ] {
            assert_eq!(
                download_flags(&["--url", name]),
                Some(DownloadFlags {
                    url: Some(variant),
                    ..DownloadFlags::default()
                })
            );
            assert_eq!(variant.label(), name);
        }
        assert_eq!(
            download_flags(&["--to", "/s", "--url", "plain", "--item", "2"]),
            Some(DownloadFlags {
                item: Some(2),
                to: Some("/s"),
                url: Some(UrlVariant::Plain)
            })
        );
        for bad in [
            &["--url", "Plain"][..],
            &["--url", ""],
            &["--url", "attach"],
            &["--url"],
            &["--url", "plain", "stray"],
        ] {
            assert_eq!(download_flags(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn each_variant_asks_for_its_own_target() {
        let id = || "disk/0a1b-2c".to_owned();
        let url = |variant: UrlVariant| {
            variant
                .target(id())
                .url(&MediaConfig::production())
                .unwrap()
        };
        assert!(url(UrlVariant::Original).ends_with("/file_shortterm/disk/0a1b-2c?attach=true"));
        assert!(url(UrlVariant::Plain).ends_with("/file_shortterm/disk/0a1b-2c"));
        assert!(url(UrlVariant::Stream).ends_with("/video_streams/disk/0a1b-2c"));
    }

    #[test]
    fn select_file_accepts_a_voice_message() {
        let voice = file("disk/v", Some("voice.ogg"));
        let body = WireBody::Voice {
            file: voice.clone(),
            duration: Some(12),
        };
        assert_eq!(
            select_file(&body, None),
            Ok(Chosen {
                file: &voice,
                position: 1,
                count: 1
            })
        );
        assert!(select_file(&body, Some(1)).is_ok());
        assert!(
            select_file(&body, Some(2))
                .unwrap_err()
                .contains("one file")
        );
    }

    #[test]
    fn sha256_matches_the_published_vectors() {
        let digest = |chunks: &[&[u8]]| {
            let mut hash = Sha256::new();
            for chunk in chunks {
                hash.update(chunk);
            }
            hash.finish()
        };
        assert_eq!(
            digest(&[]),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            digest(&[b"abc"]),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // 56 bytes: the length field does not fit in the first block.
        assert_eq!(
            digest(&[b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"]),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // The same message split at odd places, across a block boundary.
        assert_eq!(
            digest(&[
                b"abcdbcdecdefdefgefghfghighijh",
                b"ijkijkljklmklmnlmnomnopnopq"
            ]),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        let chunk = [b'a'; 1000];
        let million: Vec<&[u8]> = (0..1000).map(|_| chunk.as_slice()).collect();
        assert_eq!(
            digest(&million),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn the_hashing_writer_hashes_what_it_wrote() {
        let mut out = Hashing {
            inner: Vec::new(),
            hash: Sha256::new(),
        };
        out.write_all(b"a").unwrap();
        out.write_all(b"bc").unwrap();
        assert_eq!(out.inner, b"abc");
        assert_eq!(
            out.hash.finish(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn only_a_private_existing_directory_is_a_sample_directory() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let set = |mode| std::fs::set_permissions(dir.path(), PermissionsExt::from_mode(mode));
        set(0o700).unwrap();
        assert_eq!(check_sample_dir(dir.path()), Ok(()));
        for mode in [0o750, 0o705, 0o755, 0o777] {
            set(mode).unwrap();
            assert!(
                check_sample_dir(dir.path()).unwrap_err().contains("0700"),
                "{mode:o}"
            );
        }
        assert!(check_sample_dir(&dir.path().join("missing")).is_err());
        let plain = dir.path().join("file");
        std::fs::write(&plain, b"x").unwrap();
        assert!(check_sample_dir(&plain).is_err());
    }

    #[test]
    fn a_sample_name_shows_no_server_name_but_a_short_extension() {
        let name = |raw: Option<&str>, position, variant| {
            let one = file("disk/0a1b-2c", raw);
            let chosen = Chosen {
                file: &one,
                position,
                count: 3,
            };
            sample_name(1700000000123456, &chosen, variant)
        };
        assert_eq!(
            name(Some("My secret voice.ogg"), 1, UrlVariant::Plain),
            "1700000000123456-1-plain.ogg"
        );
        assert_eq!(
            name(Some("clip.MP4"), 2, UrlVariant::Stream),
            "1700000000123456-2-stream.MP4"
        );
        for unusable in [
            Some("noextension"),
            Some("a.toolongextension"),
            Some("a.o g"),
            Some("a.ö"),
            Some("a."),
            Some(".hidden"),
            None,
        ] {
            assert_eq!(
                name(unusable, 1, UrlVariant::Original),
                "1700000000123456-1-original",
                "{unusable:?}"
            );
        }
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;

    const ALL: [SearchEntity; 3] = [
        SearchEntity::Messages,
        SearchEntity::Users,
        SearchEntity::Chats,
    ];

    #[test]
    fn search_defaults_to_every_entity_and_the_app_limit() {
        assert_eq!(
            search_flags(&[]),
            Some(SearchFlags {
                entities: ALL.to_vec(),
                limit: SEARCH_LIMIT
            })
        );
    }

    #[test]
    fn search_flags_parse_in_any_order() {
        let expected = Some(SearchFlags {
            entities: vec![SearchEntity::Chats, SearchEntity::Messages],
            limit: 3,
        });
        assert_eq!(
            search_flags(&["--entities", "chats,messages", "--limit", "3"]),
            expected
        );
        assert_eq!(
            search_flags(&["--limit", "3", "--entities", "chats,messages"]),
            expected
        );
        assert_eq!(
            search_flags(&["--limit", "25"]),
            Some(SearchFlags {
                entities: ALL.to_vec(),
                limit: 25
            })
        );
    }

    #[test]
    fn search_flags_reject_unknown_repeated_and_malformed_input() {
        for bad in [
            &["--entities", "contacts"][..],
            &["--entities", "messages,contacts"],
            &["--entities", "messages,messages"],
            &["--entities", ""],
            &["--entities", "messages,"],
            &["--entities"],
            &["--limit", "0"],
            &["--limit", "-1"],
            &["--limit", "x"],
            &["--limit"],
            &["--limit", "1", "--limit", "2"],
            &["--entities", "users", "--entities", "chats"],
            &["--page", "2"],
            &["--limit", "1", "stray"],
        ] {
            assert_eq!(search_flags(bad), None, "{bad:?}");
        }
    }
}

#[cfg(test)]
mod reactors_tests {
    use super::*;

    fn parsed(args: &[&str]) -> Option<ReactorsArgs> {
        reactors_args(args)
    }

    #[test]
    fn reactors_takes_a_chat_and_a_timestamp_and_defaults_to_the_first_page() {
        assert_eq!(
            parsed(&["chat-1", "1700000000000000"]),
            Some(ReactorsArgs {
                chat: "chat-1".to_owned(),
                timestamp: 1_700_000_000_000_000,
                before: None,
                limit: REACTORS_PAGE,
            })
        );
    }

    #[test]
    fn reactors_flags_parse_in_any_order() {
        let expected = Some(ReactorsArgs {
            chat: "c".to_owned(),
            timestamp: 5,
            before: Some(3),
            limit: 7,
        });
        assert_eq!(
            parsed(&["c", "5", "--before", "3", "--limit", "7"]),
            expected
        );
        assert_eq!(
            parsed(&["c", "5", "--limit", "7", "--before", "3"]),
            expected
        );
        assert_eq!(
            parsed(&["c", "5", "--limit", "7"]),
            Some(ReactorsArgs {
                chat: "c".to_owned(),
                timestamp: 5,
                before: None,
                limit: 7,
            })
        );
    }

    #[test]
    fn a_missing_timestamp_is_a_usage_error() {
        assert_eq!(parsed(&[]), None);
        assert_eq!(parsed(&["c"]), None);
        assert_eq!(parsed(&["c", "--limit", "7"]), None);
    }

    #[test]
    fn bad_numbers_and_unknown_or_repeated_flags_are_usage_errors() {
        for bad in [
            &["c", "x"][..],
            &["c", ""],
            &["c", "1.5"],
            &["c", "5", "--before"],
            &["c", "5", "--before", "x"],
            &["c", "5", "--before", "1", "--before", "2"],
            &["c", "5", "--limit"],
            &["c", "5", "--limit", "0"],
            &["c", "5", "--limit", "-1"],
            &["c", "5", "--limit", "x"],
            &["c", "5", "--limit", "1", "--limit", "2"],
            &["c", "5", "--mode", "0"],
            &["c", "5", "--page", "2"],
            &["c", "5", "stray"],
            &["c", "5", "--limit", "1", "stray"],
        ] {
            assert_eq!(parsed(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_usage_text_lists_reactors_as_a_read() {
        assert!(
            USAGE.contains("bt-probe reactors <chat> <micros> [--before <micros>] [--limit <n>]")
        );
        assert!(USAGE.contains("reactors is read-only"));
        assert!(USAGE.contains("BT_WRITE_ONLY_CHAT does not apply"));
    }
}
