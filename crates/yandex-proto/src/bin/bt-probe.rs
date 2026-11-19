//! Developer probe for the Yandex protocol layer. Read-only; never prints cookie values.

use std::process::ExitCode;
use std::time::Duration;

use yandex_proto::bootstrap::{RegistryEndpoints, bootstrap};
use yandex_proto::runtime::RuntimeDir;
use yandex_proto::session::Session;
use yandex_proto::wire::{WireBody, WireMessage};
use yandex_proto::xiva::{XivaEvent, XivaEvents};
use yandex_proto::{Client, ClientConfig, ProtoError};

const USAGE: &str = "usage: bt-probe me | chats | history <chat> [--before <micros>] | listen [--raw] [--subscribe]";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

type Failure = (u8, String);

fn main() -> ExitCode {
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
        ["listen", flags @ ..] if flags.iter().all(|f| ["--raw", "--subscribe"].contains(f)) => {
            block_on(listen(flags.contains(&"--raw"), flags.contains(&"--subscribe")))
        }
        _ => return usage(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, message)) => {
            eprintln!("bt-probe: {message}");
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

async fn me() -> Result<(), Failure> {
    let session = load_session()?;
    let identity = bootstrap(&RegistryEndpoints::production(), &session.cookies).await.map_err(|e| (1, e.to_string()))?;
    println!("uid: {}", identity.uid.as_deref().unwrap_or("-"));
    println!("guid: {}", identity.guid);
    println!("display name: {}", identity.display_name.as_deref().unwrap_or("-"));
    Ok(())
}

/// Starts the client and waits until the push socket is subscribed, so requests do not fail offline.
async fn connect(config: ClientConfig) -> Result<(Client, XivaEvents), Failure> {
    let session = load_session()?;
    let (client, mut events) = Client::start(&session, config).map_err(fail)?;
    let wait = async {
        loop {
            match events.recv().await {
                Some(XivaEvent::Connected { subscription_id }) => {
                    println!("connected, subscription-id: {subscription_id}");
                    return Ok(());
                }
                Some(XivaEvent::SessionExpired) => return Err(fail(ProtoError::SessionExpired)),
                Some(XivaEvent::Disconnected { reason, retry_in }) => {
                    eprintln!("disconnected: {reason} (retry in {}s)", retry_in.as_secs());
                }
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
    let (client, _events) = connect(ClientConfig::production()).await?;
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
    let (client, _events) = connect(ClientConfig::production()).await?;
    let page = client.history(chat, before).await.map_err(fail)?;
    for message in &page.messages {
        println!("{}", describe(message));
    }
    println!("-- {} messages, reached_start={}, skipped={}", page.messages.len(), page.reached_start, page.skipped);
    Ok(())
}

async fn listen(raw: bool, subscribe: bool) -> Result<(), Failure> {
    let mut config = ClientConfig::production();
    config.xiva.raw = raw;
    let (client, mut events) = connect(config).await?;
    if subscribe {
        let chats = client.chat_list().await.map_err(fail)?;
        println!("subscribing {} chats", chats.len());
        client.watch_chats(chats.into_iter().map(|c| c.id).collect());
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
    format!("{}\t{author}\t{body}{flags}", message.timestamp)
}
