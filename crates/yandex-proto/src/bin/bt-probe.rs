//! Developer probe for the Yandex protocol layer. Reads, plus a few writes that act on the
//! account's self chat only; never prints cookie values.
//! Command output goes to stdout; diagnostics go through `tracing` to stderr (filter: `BT_LOG`).

use std::io::IsTerminal;
use std::process::ExitCode;
use std::time::Duration;

use tracing::error;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use yandex_proto::bootstrap::{RegistryEndpoints, bootstrap};
use yandex_proto::runtime::RuntimeDir;
use yandex_proto::session::Session;
use yandex_proto::wire::{Relation, WireBody, WireMessage};
use yandex_proto::write::{Committed, ReplyRef, WriteRequest};
use yandex_proto::xiva::{XivaEvent, XivaEvents};
use yandex_proto::{Client, ClientConfig, ProtoError};

const USAGE: &str = "usage: bt-probe me | chats | history <chat> [--before <micros>] | listen [--watch <chat>]
       bt-probe send-self <text> [--payload <id>] [--reply <micros>] [--chat <self chat>]
       bt-probe edit-self <micros> <text> [--reply <micros>] [--chat <self chat>]
       bt-probe delete-self <micros> [--chat <self chat>]
       bt-probe typing-self [--chat <self chat>] | seen <chat> <micros>
the *-self commands refuse any chat but the account's own self chat
log filter: BT_LOG (default info), e.g. BT_LOG=yandex_proto=debug or BT_LOG=yandex_proto::xiva=trace";
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
        ["typing-self", flags @ ..] => match chat_flag(flags) {
            Some(chat) => block_on(write_self(chat, |chat| WriteRequest::Typing { chat })),
            None => return usage(),
        },
        ["seen", chat, ts] => match ts.parse() {
            Ok(ts) => block_on(seen(chat, ts)),
            Err(_) => return usage(),
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
        .ok_or_else(|| (1, format!("no message {timestamp} in the self chat")))
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

/// Delete and typing: one push into the self chat.
async fn write_self(requested: Option<&str>, build: impl FnOnce(String) -> WriteRequest) -> Result<(), Failure> {
    let session = load_session()?;
    let chat = resolve_self_chat(requested, &session.identity.guid).map_err(|e| (2, e))?;
    let (client, _events) = connect(&session, ClientConfig::production()).await?;
    print_committed(&client.push(build(chat)).await.map_err(fail)?);
    Ok(())
}

/// Moves the read position of any chat; the caller names the chat on purpose.
async fn seen(chat: &str, timestamp: i64) -> Result<(), Failure> {
    let session = load_session()?;
    let (client, _events) = connect(&session, ClientConfig::production()).await?;
    print_committed(&client.push(WriteRequest::Seen { chat: chat.to_owned(), timestamp }).await.map_err(fail)?);
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
    format!("{}\t{author}\t{body}{flags}{reply}", message.timestamp)
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
}
