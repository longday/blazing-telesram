//! Developer probe for the Yandex protocol layer. Reads, plus a few writes that act on the
//! account's self chat only (send, edit, delete, typing, and `react-self`, which sets, replaces or
//! removes the account's own reaction on a self-chat message); never prints cookie values.
//! `listen` prints reaction pushes through `Debug` like every other push and `history` lists the
//! reactions of a message after its text. `download` saves one file or image
//! of any chat into a local directory (never overwriting) and prints host and path of the
//! request, never its query. `create-private` and `create-chat` are the commands that change the
//! account outside the self chat, and they send nothing without `--confirm-create`.
//! Command output goes to stdout; diagnostics go through `tracing` to stderr (filter: `BT_LOG`).

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, IsTerminal};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::Duration;

use tracing::error;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use yandex_proto::bootstrap::{RegistryEndpoints, bootstrap};
use yandex_proto::media::{MediaConfig, MediaTarget};
use yandex_proto::request::{MEMBERS_PAGE, SEARCH_LIMIT, SearchEntity};
use yandex_proto::runtime::RuntimeDir;
use yandex_proto::search::SearchSet;
use yandex_proto::session::Session;
use yandex_proto::wire::{Relation, WireBody, WireFile, WireMessage};
use yandex_proto::write::{Committed, ReactionChange, ReplyRef, WriteRequest};
use yandex_proto::xiva::{XivaEvent, XivaEvents};
use yandex_proto::{Client, ClientConfig, MAX_CHAT_TITLE, ManageError, ProtoError};

const USAGE: &str = "usage: bt-probe me | chats | history <chat> [--before <micros>] | listen [--watch <chat>]
       bt-probe send-self <text> [--payload <id>] [--reply <micros>] [--chat <self chat>]
       bt-probe edit-self <micros> <text> [--reply <micros>] [--chat <self chat>]
       bt-probe delete-self <micros> [--chat <self chat>]
       bt-probe react-self <micros> <code|remove> [--chat <self chat>]
       bt-probe typing-self [--chat <self chat>] | seen <chat> <micros> | typing <chat>
       bt-probe download <chat> <micros> [--item N] [--to <dir>]
       bt-probe search <query> [--entities messages,users,chats] [--limit <n>]
       bt-probe whoami | members <chat>
       bt-probe create-private <guid> --confirm-create
       bt-probe create-chat <title> <guid>... --confirm-create
the *-self commands refuse any chat but the account's own self chat
download takes the file or the N-th (from 1) image of the message with that timestamp, saves it
into <dir> (default: the current directory) and refuses to overwrite an existing file
log filter: BT_LOG (default info), e.g. BT_LOG=yandex_proto=debug or BT_LOG=yandex_proto::xiva=trace
search is read-only: per entity it prints total/limit/page/pages and the raw items, one JSON per line (the query is not echoed or logged above trace; items carry names and message text)
react-self puts the reaction <code> (a positive integer, the web client's reaction type) on the self-chat message with that timestamp, or takes the account's reaction away with `remove`; it sends one write push and prints the same status line as the other *-self commands. history lists a row's reactions after its text: code x count, the version, the recent reactors (user:code)
whoami and members are read-only and print the parsed reply (`{:#?}`) on stdout (guids and display names; the header line goes to stderr). members asks get_chat_members for the first page of 50 users and answers group chats only
create-private opens (creating it when missing) the private chat with the user <guid> on the real account, once and never retried; without --confirm-create it prints the target and exits with code 2 without any request. Exit codes: 0 created, 1 not sent or rejected (nothing was created), 3 outcome unknown (the chat may exist: check the web client before repeating)
create-chat creates a group chat named <title> with the listed participants (guids, not your own) on the real account, once and never retried; the payload is the web client's own (description empty, not a channel, is_public true), not yet confirmed against the server. Without --confirm-create it prints the title and participants it would send and exits with code 2 without any request; the title must be non-blank and at most 250 characters, every guid distinct. Same exit codes as create-private; a created group whose reply lists participants the server could not add still exits 0 and names them";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

type Failure = (u8, String);

/// Installs the stderr logger; the error names `BT_LOG` when its value is not a filter.
fn init_logging() -> Result<(), String> {
    tracing_subscriber::registry()
        .with(yandex_proto::log::filter_from_env()?)
        .with(yandex_proto::log::dependency_cap())
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr).with_ansi(std::io::stderr().is_terminal()))
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
            (Ok(ts), Some(chat)) => block_on(write_self(chat, move |chat| WriteRequest::Delete { chat, timestamp: ts })),
            _ => return usage(),
        },
        ["react-self", ts, change, flags @ ..] => match (ts.parse(), reaction_change(change), chat_flag(flags)) {
            (Ok(ts), Some(change), Some(chat)) => {
                block_on(write_self(chat, move |chat| WriteRequest::Reaction { chat, timestamp: ts, change }))
            }
            _ => return usage(),
        },
        ["typing-self", flags @ ..] => match chat_flag(flags) {
            Some(chat) => block_on(write_self(chat, |chat| WriteRequest::Typing { chat })),
            None => return usage(),
        },
        ["seen", chat, ts] => match ts.parse() {
            Ok(ts) => block_on(write_named(WriteRequest::Seen { chat: (*chat).to_owned(), timestamp: ts })),
            Err(_) => return usage(),
        },
        ["typing", chat] => block_on(write_named(WriteRequest::Typing { chat: (*chat).to_owned() })),
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
    tokio::runtime::Runtime::new().map_err(|e| (1, e.to_string()))?.block_on(future)
}

fn fail(e: ProtoError) -> Failure {
    match e {
        ProtoError::SessionExpired => (1, "session expired: sign in again via ./run.sh".to_owned()),
        other => (1, other.to_string()),
    }
}

fn load_session() -> Result<Session, Failure> {
    let dir = RuntimeDir::from_env().map_err(|e| (2, e.to_string()))?;
    Session::load(&dir)
        .map_err(|e| (1, e.to_string()))?
        .ok_or_else(|| (1, "no stored session: sign in first via ./run.sh".to_owned()))
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
        Some(chat) if chat != own => Err(format!("refusing {chat}: the *-self commands act on the self chat only")),
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
    arg.parse::<i64>().ok().filter(|code| *code > 0).map(ReactionChange::Set)
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

/// Edits take the same flags as sends minus the payload id. Without `--reply` an edit repeats the
/// reply the message already has; `--reply` re-attaches one to a message that lost it.
fn edit_flags<'a>(flags: &[&'a str]) -> Option<SendFlags<'a>> {
    send_flags(flags).filter(|f| f.payload.is_none())
}

fn print_committed(committed: &Committed) {
    let outcome = if committed.duplicate() { "DUPLICATE" } else { "FULLY_COMMITTED" };
    print!("status={} {outcome}", committed.status);
    if let Some(info) = &committed.message {
        print!(" timestamp={} seq_no={:?} version={:?}", info.timestamp_mcs, info.seq_no, info.version);
    }
    println!();
}

async fn send_self(text: &str, flags: SendFlags<'_>) -> Result<(), Failure> {
    let session = load_session()?;
    let chat = resolve_self_chat(flags.chat, &session.identity.guid).map_err(|e| (2, e))?;
    let (client, _events) = connect(&session, ClientConfig::production()).await?;
    let reply = match flags.reply {
        Some(timestamp) => Some(reply_ref(&client, &chat, timestamp).await?),
        None => None,
    };
    let payload_id = flags.payload.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
    println!("payload: {payload_id}");
    let request = WriteRequest::SendText { chat, payload_id, text: text.to_owned(), reply };
    print_committed(&client.push(request).await.map_err(fail)?);
    Ok(())
}

/// A message of the chat, from the page that ends with it (`before` is exclusive).
async fn find_message(client: &Client, chat: &str, timestamp: i64) -> Result<WireMessage, Failure> {
    let page = client.history(chat, Some(timestamp + 1)).await.map_err(fail)?;
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
        Relation::Forward { .. } => Err(format!("message {timestamp} is a forward; editing it would drop the forward")),
        Relation::Reply { quote, reference, original } => {
            let target = reference.as_ref().and_then(|r| r.timestamp).or_else(|| original.as_ref()?.timestamp);
            let quote = quote.clone().or_else(|| match &original.as_ref()?.body {
                WireBody::Text(text) => Some(text.clone()),
                _ => None,
            });
            match (target, quote) {
                (Some(timestamp), Some(quote)) => Ok(Some(ReplyRef { timestamp, quote })),
                _ => Err(format!("the reply of message {timestamp} has no known target or quote; pass --reply")),
            }
        }
    }
}

async fn edit_self(timestamp: i64, text: &str, flags: SendFlags<'_>) -> Result<(), Failure> {
    let session = load_session()?;
    let chat = resolve_self_chat(flags.chat, &session.identity.guid).map_err(|e| (2, e))?;
    let (client, _events) = connect(&session, ClientConfig::production()).await?;
    let reply = match flags.reply {
        Some(target) => Some(reply_ref(&client, &chat, target).await?),
        None => stored_reply(&find_message(&client, &chat, timestamp).await?).map_err(|e| (1, e))?,
    };
    print_committed(&client.push(WriteRequest::Edit { chat, timestamp, text: text.to_owned(), reply }).await.map_err(fail)?);
    Ok(())
}

/// Delete, typing and reaction: one push into the self chat.
async fn write_self(requested: Option<&str>, build: impl FnOnce(String) -> WriteRequest) -> Result<(), Failure> {
    let session = load_session()?;
    let chat = resolve_self_chat(requested, &session.identity.guid).map_err(|e| (2, e))?;
    let (client, _events) = connect(&session, ClientConfig::production()).await?;
    print_committed(&client.push(build(chat)).await.map_err(fail)?);
    Ok(())
}

/// Read marker or typing in any chat; the caller names the chat on purpose. Neither carries text.
async fn write_named(request: WriteRequest) -> Result<(), Failure> {
    let session = load_session()?;
    let (client, _events) = connect(&session, ClientConfig::production()).await?;
    print_committed(&client.push(request).await.map_err(fail)?);
    Ok(())
}

async fn me() -> Result<(), Failure> {
    let session = load_session()?;
    let identity = bootstrap(&RegistryEndpoints::production(), &session.cookies).await.map_err(|e| (1, e.to_string()))?;
    println!("uid: {}", identity.uid.as_deref().unwrap_or("-"));
    println!("guid: {}", identity.guid);
    println!("display name: {}", identity.display_name.as_deref().unwrap_or("-"));
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
        let preview = chat.last_message.as_ref().map(describe).unwrap_or_default();
        println!(
            "{}\t{:?}\t{}\tunread={}{}\tts={}\t{preview}",
            chat.id,
            chat.kind,
            chat.title.as_deref().unwrap_or("-"),
            chat.unread,
            if chat.muted { " muted" } else { "" },
            chat.last_activity.map_or_else(|| "-".to_owned(), |t| t.to_string()),
        );
    }
    Ok(())
}

async fn history(chat: &str, before: Option<i64>) -> Result<(), Failure> {
    let (client, _events) = connect(&load_session()?, ClientConfig::production()).await?;
    let page = client.history(chat, before).await.map_err(fail)?;
    for message in &page.messages {
        println!("{}", describe(message));
    }
    println!("-- {} messages, reached_start={}, skipped={}", page.messages.len(), page.reached_start, page.skipped);
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
            "--limit" if limit.is_none() => limit = Some(value.parse::<u32>().ok().filter(|n| *n > 0)?),
            _ => return None,
        }
        flags = rest;
    }
    flags.is_empty().then(|| SearchFlags {
        entities: entities.unwrap_or_else(|| vec![SearchEntity::Messages, SearchEntity::Users, SearchEntity::Chats]),
        limit: limit.unwrap_or(SEARCH_LIMIT),
    })
}

/// A comma-separated, duplicate-free list of entity names.
fn search_entities(list: &str) -> Option<Vec<SearchEntity>> {
    let mut entities = Vec::new();
    for name in list.split(',') {
        let entity = [SearchEntity::Messages, SearchEntity::Users, SearchEntity::Chats].into_iter().find(|e| e.wire_name() == name)?;
        if entities.contains(&entity) {
            return None;
        }
        entities.push(entity);
    }
    Some(entities)
}

fn print_search_set(entity: SearchEntity, set: &SearchSet) {
    println!("{}: total={} limit={} page={} pages={}", entity.wire_name(), set.total, set.limit, set.page, set.pages);
    for item in &set.items {
        println!("{item}");
    }
}

/// One registry `search` (HTTP, bare cookie): the first page of every requested entity.
async fn search(query: &str, flags: SearchFlags) -> Result<(), Failure> {
    let (client, _events) = Client::start(&load_session()?, ClientConfig::production()).map_err(fail)?;
    let reply = client.search_with(query, flags.limit, &flags.entities).await.map_err(fail)?;
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
    let (client, _events) = Client::start(&load_session()?, ClientConfig::production()).map_err(fail)?;
    eprintln!("members of {chat} via get_chat_members");
    println!("{:#?}", client.members(chat, MEMBERS_PAGE, None).await.map_err(fail)?);
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
        ManageError::Rejected(e) => (1, format!("rejected by the server, nothing was created: {e}")),
        ManageError::Unknown(e) => (3, format!("outcome unknown, the chat may exist: check the web client before repeating: {e}")),
    }
}

/// One `create_private_chat` (registry mutation with CSRF, sent once, never retried). Reached
/// only with `--confirm-create`. The raw reply is visible at `BT_LOG=yandex_proto::registry=trace`.
async fn create_private(guid: &str) -> Result<(), Failure> {
    let (client, _events) = Client::start(&load_session()?, ClientConfig::production()).map_err(fail)?;
    eprintln!("creating the private chat with {guid}");
    match client.create_private_chat(guid).await.map_err(creation_failure)? {
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
    (!users.is_empty()).then_some(GroupPlan { title, users, confirmed })
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
    let (client, _events) = Client::start(&load_session()?, ClientConfig::production()).map_err(fail)?;
    eprintln!("creating the group chat with {} participant(s)", plan.users.len());
    let users: Vec<String> = plan.users.iter().map(|user| (*user).to_owned()).collect();
    let created = client.create_chat(plan.title, &users).await.map_err(creation_failure)?;
    match created.chat {
        Some(chat) => println!("created: chat {chat}"),
        None => println!("created: the reply names no chat id (look in the chat list)"),
    }
    if !created.refused.is_empty() {
        println!("not added by the server ({}): {}", created.refused.len(), created.refused.join(", "));
    }
    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DownloadFlags<'a> {
    /// 1-based position inside a gallery.
    item: Option<usize>,
    to: Option<&'a str>,
}

fn download_flags<'a>(mut flags: &[&'a str]) -> Option<DownloadFlags<'a>> {
    let mut parsed = DownloadFlags::default();
    while let [name, value, rest @ ..] = flags {
        match *name {
            "--item" => parsed.item = Some(value.parse().ok().filter(|n| *n >= 1)?),
            "--to" => parsed.to = Some(value),
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
        None | Some(1) => Ok(Chosen { file, position: 1, count: 1 }),
        Some(n) => Err(format!("--item {n}: the message holds one file")),
    };
    match body {
        WireBody::File(file) => single(file),
        WireBody::Image(image) => single(&image.file),
        WireBody::Gallery { items, .. } => {
            let position = item.unwrap_or(1);
            let image = position.checked_sub(1).and_then(|index| items.get(index));
            let image = image.ok_or_else(|| format!("--item {position}: the gallery holds {} images", items.len()))?;
            Ok(Chosen { file: &image.file, position, count: items.len() })
        }
        _ => Err("the message holds no file or image".to_owned()),
    }
}

/// The local name for a downloaded file. The server's name is untrusted: only its last path
/// component survives, without control characters, colons and leading dots; an unusable name
/// falls back to the last segment of the file id (which `MediaTarget::url` has validated).
fn local_name(file: &WireFile) -> String {
    const MAX_BYTES: usize = 200;
    let last = file.name.as_deref().and_then(|name| name.rsplit(['/', '\\']).next()).unwrap_or("");
    let cleaned: String = last.chars().map(|c| if c.is_control() || c == ':' { '_' } else { c }).collect();
    let mut name = cleaned.trim().trim_start_matches('.');
    while name.len() > MAX_BYTES {
        let mut cut = MAX_BYTES;
        while !name.is_char_boundary(cut) {
            cut -= 1;
        }
        name = &name[..cut];
    }
    if name.is_empty() { file.id.rsplit('/').next().unwrap_or("file").to_owned() } else { name.to_owned() }
}

/// Host and path of a URL: no scheme, no query, no fragment.
fn host_and_path(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['?', '#']).next().unwrap_or(rest)
}

/// Creates `name` in `dir`; an existing entry of that name is an error and is left untouched.
fn create_destination(dir: &Path, name: &str) -> Result<File, String> {
    let path = dir.join(name);
    OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| match e.kind() {
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
    let dir = Path::new(flags.to.unwrap_or("."));
    if !dir.is_dir() {
        return Err((2, format!("--to {}: not a directory", dir.display())));
    }
    let (client, _events) = connect(&load_session()?, ClientConfig::production()).await?;
    let message = find_message(&client, chat, timestamp).await?;
    let chosen = select_file(&message.body, flags.item).map_err(|e| (1, format!("message {timestamp}: {e}")))?;
    let target = MediaTarget::Original { file_id: chosen.file.id.clone() };
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
    println!("message {timestamp}: item {} of {}", chosen.position, chosen.count);
    println!("saved: {}", destination.display());
    println!("received: {received} bytes");
    println!("announced size (FileInfo.Size): {}", or_dash(chosen.file.size));
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
    let author = message.from.as_ref().and_then(|f| f.display_name.as_deref()).unwrap_or("?");
    let body = match &message.body {
        WireBody::Text(text) => text.clone(),
        other => format!("[{other:?}]"),
    };
    let flags = if message.edited { " (edited)" } else { "" };
    let reply = match &message.relation {
        Relation::Reply { quote, original, .. } => {
            let target = original.as_ref().and_then(|o| o.timestamp).map_or_else(|| "?".to_owned(), |t| t.to_string());
            format!(" (reply to {target}, quote {:?})", quote.as_deref().unwrap_or(""))
        }
        _ => String::new(),
    };
    format!("{}\t{author}\t{body}{flags}{reply}{}", message.timestamp, reactions_note(message))
}

/// The reaction state of a history row, as the server sent it: counts `code×count`, the version
/// and the recent reactors' codes. Empty when the row carried none of the three siblings.
fn reactions_note(message: &WireMessage) -> String {
    let Some(reactions) = &message.reactions else { return String::new() };
    let counts: Vec<String> = reactions.counts.iter().map(|c| format!("{}x{}", c.code, c.count)).collect();
    let recent: Vec<String> = reactions
        .recent
        .iter()
        .map(|r| format!("{}:{}", r.user.as_deref().unwrap_or("?"), r.code))
        .collect();
    let version = reactions.version.map_or_else(|| "-".to_owned(), |v| v.to_string());
    format!(" (reactions [{}], version {version}, recent [{}])", counts.join(" "), recent.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use yandex_proto::wire::{WireForwarded, WireRef};

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
            relation,
            reactions: None,
            raw: serde_json::Value::Null,
        }
    }

    fn original(timestamp: Option<i64>, body: WireBody) -> WireForwarded {
        WireForwarded { chat: None, timestamp, from: None, body }
    }

    #[test]
    fn an_edit_repeats_the_stored_quote_not_the_current_text_of_the_original() {
        // The original was edited after the reply: the quote still shows the old text.
        let edited_since = Relation::Reply {
            quote: Some("old text".into()),
            reference: None,
            original: Some(original(Some(10), WireBody::Text("new text".into()))),
        };
        assert_eq!(stored_reply(&message(edited_since)), Ok(Some(ReplyRef { timestamp: 10, quote: "old text".into() })));
        // The original was deleted: no original body, the quote and target still come from the reply.
        let deleted = Relation::Reply { quote: Some("old text".into()), reference: None, original: Some(original(Some(10), WireBody::Empty)) };
        assert_eq!(stored_reply(&message(deleted)), Ok(Some(ReplyRef { timestamp: 10, quote: "old text".into() })));
        // A reference on the message wins over the snapshot for the target.
        let referenced = Relation::Reply {
            quote: Some("q".into()),
            reference: Some(WireRef { chat: None, timestamp: Some(7) }),
            original: Some(original(Some(10), WireBody::Empty)),
        };
        assert_eq!(stored_reply(&message(referenced)), Ok(Some(ReplyRef { timestamp: 7, quote: "q".into() })));
    }

    #[test]
    fn stored_reply_falls_back_to_the_snapshot_text_and_refuses_what_it_cannot_keep() {
        let no_quote = Relation::Reply { quote: None, reference: None, original: Some(original(Some(10), WireBody::Text("orig".into()))) };
        assert_eq!(stored_reply(&message(no_quote)), Ok(Some(ReplyRef { timestamp: 10, quote: "orig".into() })));
        assert_eq!(stored_reply(&message(Relation::None)), Ok(None));
        let unknown_target = Relation::Reply { quote: Some("q".into()), reference: None, original: None };
        assert!(stored_reply(&message(unknown_target)).unwrap_err().contains("--reply"));
        let nothing_to_quote = Relation::Reply { quote: None, reference: None, original: Some(original(Some(10), WireBody::Empty)) };
        assert!(stored_reply(&message(nothing_to_quote)).is_err());
        assert!(stored_reply(&message(Relation::Forward { originals: vec![] })).unwrap_err().contains("forward"));
    }

    #[test]
    fn self_commands_act_on_the_self_chat_and_refuse_every_other_chat() {
        assert_eq!(resolve_self_chat(None, GUID).unwrap(), format!("{GUID}_{GUID}"));
        assert_eq!(resolve_self_chat(Some(&format!("{GUID}_{GUID}")), GUID).unwrap(), format!("{GUID}_{GUID}"));
        for other in ["0/0/some-group", "dc8fe606-16f9-97d9-4faf-3acb552a7764_c9a71201-70b5-992e-61f2-b27e2d3fc8fb", GUID, ""] {
            let error = resolve_self_chat(Some(other), GUID).unwrap_err();
            assert!(error.contains("self chat only"), "{other}: {error}");
        }
    }

    #[test]
    fn send_flags_parse_in_any_order_and_reject_the_rest() {
        assert_eq!(send_flags(&[]), Some(SendFlags::default()));
        assert_eq!(
            send_flags(&["--reply", "17", "--payload", "p-1"]),
            Some(SendFlags { payload: Some("p-1"), reply: Some(17), chat: None })
        );
        assert_eq!(send_flags(&["--chat", "c"]), Some(SendFlags { chat: Some("c"), ..SendFlags::default() }));
        for bad in [&["--reply", "x"][..], &["--payload"], &["--unknown", "1"], &["--reply", "1", "stray"]] {
            assert_eq!(send_flags(bad), None, "{bad:?}");
        }
        assert_eq!(edit_flags(&["--reply", "17"]), Some(SendFlags { reply: Some(17), ..SendFlags::default() }));
        assert_eq!(edit_flags(&["--payload", "p"]), None);
        assert_eq!(chat_flag(&[]), Some(None));
        assert_eq!(chat_flag(&["--chat", "c"]), Some(Some("c")));
        assert_eq!(chat_flag(&["c"]), None);
    }

    #[test]
    fn react_self_takes_a_positive_code_or_remove() {
        assert_eq!(reaction_change("100101"), Some(ReactionChange::Set(100_101)));
        assert_eq!(reaction_change("remove"), Some(ReactionChange::Remove));
        for bad in ["", "0", "-5", "x", "1.5", "Remove", "remove ", "100101 ", "99999999999999999999"] {
            assert_eq!(reaction_change(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn react_self_refuses_a_chat_other_than_the_self_chat() {
        // The command goes through `write_self`, so `resolve_self_chat` is what guards the chat.
        assert_eq!(chat_flag(&["--chat", "c"]), Some(Some("c")));
        let error = resolve_self_chat(Some("0/0/some-group"), GUID).unwrap_err();
        assert!(error.contains("self chat only"), "{error}");
        assert_eq!(resolve_self_chat(None, GUID).unwrap(), format!("{GUID}_{GUID}"));
    }

    #[test]
    fn a_history_row_lists_its_reactions_only_when_it_has_them() {
        use yandex_proto::wire::{WireReactionCount, WireReactions, WireReactor};
        let mut row = message(Relation::None);
        assert_eq!(describe(&row), "20\t?\tmine");
        row.reactions = Some(WireReactions {
            counts: vec![WireReactionCount { code: 100_101, count: 2 }],
            version: Some(3),
            recent: vec![WireReactor { user: Some("u".into()), code: 100_101, timestamp: None }],
        });
        assert_eq!(describe(&row), "20\t?\tmine (reactions [100101x2], version 3, recent [u:100101])");
        row.reactions = Some(WireReactions { version: Some(4), ..WireReactions::default() });
        assert_eq!(describe(&row), "20\t?\tmine (reactions [], version 4, recent [])");
    }

    #[test]
    fn create_private_refuses_without_the_flag_and_names_the_target() {
        assert_eq!(create_private_args(GUID, &[]), Some(CreatePrivate::Refuse(GUID)));
        assert_eq!(create_private_args(GUID, &["--confirm-create"]), Some(CreatePrivate::Send(GUID)));
        for bad in [&["--confirm"][..], &["--confirm-create", "--confirm-create"], &["--confirm-create", "x"], &["yes"]] {
            assert_eq!(create_private_args(GUID, bad), None, "{bad:?}");
        }
        assert_eq!(create_private_args("  ", &["--confirm-create"]), None);
        assert_eq!(create_private_args("", &[]), None);
        let message = refusal(GUID);
        assert!(message.contains(GUID) && message.contains("nothing was sent") && message.contains("--confirm-create"), "{message}");
    }

    #[test]
    fn create_chat_refuses_without_the_flag_and_prints_what_it_would_send() {
        let plan = create_chat_args("Team A", &[GUID, "g-2"]).unwrap();
        assert_eq!(plan, GroupPlan { title: "Team A", users: vec![GUID, "g-2"], confirmed: false });
        let message = group_refusal(&plan);
        for needle in ["\"Team A\"", GUID, "g-2", "is_public: true", "channel: false", "nothing was sent", "--confirm-create"] {
            assert!(message.contains(needle), "{needle}: {message}");
        }
        // The flag may stand anywhere among the guids.
        for rest in [&[GUID, "g-2", "--confirm-create"][..], &["--confirm-create", GUID, "g-2"], &[GUID, "--confirm-create", "g-2"]] {
            assert_eq!(create_chat_args("Team A", rest), Some(GroupPlan { title: "Team A", users: vec![GUID, "g-2"], confirmed: true }));
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
        WireFile { id: id.into(), name: name.map(Into::into), size: Some(5) }
    }

    fn image(id: &str) -> WireImage {
        WireImage { file: file(id, None), width: 1, height: 1, animated: false }
    }

    #[test]
    fn download_flags_parse_in_any_order_and_reject_the_rest() {
        assert_eq!(download_flags(&[]), Some(DownloadFlags::default()));
        assert_eq!(download_flags(&["--to", "/tmp/x", "--item", "3"]), Some(DownloadFlags { item: Some(3), to: Some("/tmp/x") }));
        assert_eq!(download_flags(&["--item", "1"]), Some(DownloadFlags { item: Some(1), to: None }));
        for bad in [&["--item", "0"][..], &["--item", "x"], &["--item", "-1"], &["--item"], &["--to"], &["--unknown", "1"], &["--item", "1", "stray"]] {
            assert_eq!(download_flags(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn select_file_takes_the_file_or_the_nth_image_and_refuses_the_rest() {
        let one = file("disk/a", Some("a.bin"));
        let body = WireBody::File(one.clone());
        assert_eq!(select_file(&body, None), Ok(Chosen { file: &one, position: 1, count: 1 }));
        assert!(select_file(&body, Some(2)).unwrap_err().contains("one file"));
        let single = WireBody::Image(image("disk/i"));
        assert_eq!(select_file(&single, Some(1)).unwrap().file.id, "disk/i");
        assert!(select_file(&single, Some(2)).is_err());
        let gallery = WireBody::Gallery { items: vec![image("disk/1"), image("disk/2"), image("disk/3")], text: None };
        let third = select_file(&gallery, Some(3)).unwrap();
        assert_eq!((third.file.id.as_str(), third.position, third.count), ("disk/3", 3, 3));
        assert_eq!(select_file(&gallery, None).unwrap().file.id, "disk/1");
        assert!(select_file(&gallery, Some(4)).unwrap_err().contains("holds 3 images"));
        assert!(select_file(&WireBody::Gallery { items: vec![], text: None }, None).is_err());
        for other in [WireBody::Text("t".into()), WireBody::Deleted, WireBody::Empty] {
            assert!(select_file(&other, None).unwrap_err().contains("no file or image"), "{other:?}");
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
        for unusable in [None, Some(""), Some("."), Some(".."), Some("dir/"), Some("  ")] {
            assert_eq!(name(unusable), "0a1b-2c", "{unusable:?}");
        }
        let long = "я".repeat(150) + ".txt";
        let cut = name(Some(&long));
        assert!(cut.len() <= 200 && cut.chars().all(|c| c == 'я'), "{} bytes", cut.len());
    }

    #[test]
    fn the_printed_target_has_host_and_path_only() {
        let url = MediaTarget::Original { file_id: "disk/0a1b-2c".into() }.url(&MediaConfig::production()).unwrap();
        assert_eq!(host_and_path(&url), "files.messenger.yandex.ru/file_shortterm/disk/0a1b-2c");
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
}

#[cfg(test)]
mod search_tests {
    use super::*;

    const ALL: [SearchEntity; 3] = [SearchEntity::Messages, SearchEntity::Users, SearchEntity::Chats];

    #[test]
    fn search_defaults_to_every_entity_and_the_app_limit() {
        assert_eq!(search_flags(&[]), Some(SearchFlags { entities: ALL.to_vec(), limit: SEARCH_LIMIT }));
    }

    #[test]
    fn search_flags_parse_in_any_order() {
        let expected = Some(SearchFlags { entities: vec![SearchEntity::Chats, SearchEntity::Messages], limit: 3 });
        assert_eq!(search_flags(&["--entities", "chats,messages", "--limit", "3"]), expected);
        assert_eq!(search_flags(&["--limit", "3", "--entities", "chats,messages"]), expected);
        assert_eq!(search_flags(&["--limit", "25"]), Some(SearchFlags { entities: ALL.to_vec(), limit: 25 }));
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
