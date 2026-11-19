mod desktop;
mod format;
mod input;
mod login;
mod store;
mod theme;
mod timeline;
mod widgets;
mod yandex_backend;
#[cfg(test)]
mod tests;

use bt_chat_core::model::UserId;
use bt_chat_core::{ChatClient, ClientConfig};
use desktop::{Desktop, Quit, Search};
use format::Clock;
use gpui::{prelude::*, *};
use login::{LoginCookie, SignInWindow};
use store::{ChatStore, SessionState, StoreEvent};
use yandex_backend::YandexBackend;
use yandex_proto::bootstrap::{BootstrapError, RegistryEndpoints, bootstrap};
use yandex_proto::runtime::RuntimeDir;
use yandex_proto::session::{Identity, Session, SessionCookie};

fn main() {
    let runtime = match RuntimeDir::from_env() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    Application::new().run(move |cx: &mut App| {
        input::bind(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-f", Search, Some("BlazingTelesram")),
            KeyBinding::new("ctrl-f", Search, Some("BlazingTelesram")),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        match Session::load(&runtime) {
            Ok(Some(session)) => open_chat_window(runtime, session, cx),
            Ok(None) => open_sign_in(runtime, cx),
            Err(error) => {
                eprintln!("cannot read the stored session: {error}");
                cx.quit();
            }
        }
        cx.activate(true);
    });
}

fn session_cookie(cookie: LoginCookie) -> SessionCookie {
    SessionCookie {
        name: cookie.name,
        value: cookie.value,
        domain: cookie.domain,
        path: cookie.path,
        expires_unix: cookie.expires_unix,
        secure: cookie.secure,
        http_only: cookie.http_only,
    }
}

/// Runs the HTTP bootstrap on a throwaway Tokio runtime: reqwest needs one, GPUI has none.
fn verify(cookies: Vec<SessionCookie>) -> async_channel::Receiver<(Vec<SessionCookie>, Result<Identity, BootstrapError>)> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let result = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime.block_on(bootstrap(&RegistryEndpoints::production(), &cookies)),
            Err(error) => Err(BootstrapError::Network(error.to_string())),
        };
        let _ = tx.send_blocking((cookies, result));
    });
    rx
}

/// Closing this window before sign-in completes quits the app without writing anything.
fn open_sign_in(runtime: RuntimeDir, cx: &mut App) {
    let opened = login::open_sign_in_window(cx, move |cookies, window, cx| {
        let Some(handle) = window.window_handle().downcast::<SignInWindow>() else { return };
        let verified = verify(cookies.into_iter().map(session_cookie).collect());
        let runtime = runtime.clone();
        cx.spawn(async move |cx| {
            let Ok((cookies, result)) = verified.recv().await else { return };
            let _ = cx.update(|cx| {
                let saved = match result {
                    Ok(identity) => {
                        let session = Session { cookies, identity };
                        session.save(&runtime).map(|()| session).map_err(|e| e.to_string())
                    }
                    Err(error) => Err(error.to_string()),
                };
                match saved {
                    Ok(session) => {
                        // Open the next window first: closing the last one quits the app.
                        open_chat_window(runtime, session, cx);
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    }
                    Err(error) => {
                        eprintln!("sign-in verification failed: {error}");
                        let _ = handle.update(cx, |view, _, _| view.resume_polling());
                    }
                }
            });
        })
        .detach();
    });
    if let Err(error) = opened {
        eprintln!("cannot open the sign-in window: {error}");
        cx.quit();
    }
}

fn open_chat_window(runtime: RuntimeDir, session: Session, cx: &mut App) {
    let account = UserId(session.identity.xiva_user().to_owned());
    let config = ClientConfig::new(runtime.path(), runtime.session_file(), account);
    let client = match ChatClient::spawn(YandexBackend::new(session), config) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("cannot start the chat client: {error}");
            return cx.quit();
        }
    };
    let events = client.events();
    // The forwarder owns the client: it stops once the store drops its command sender.
    let (commands, command_rx) = async_channel::unbounded();
    std::thread::spawn(move || {
        while let Ok(command) = command_rx.recv_blocking() {
            client.send(command);
        }
    });
    let store = cx.new(|cx| ChatStore::new(events, commands, cx));
    let bounds = Bounds::centered(None, size(px(1180.), px(800.)), cx);
    let window_store = store.clone();
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(850.), px(560.))),
            titlebar: Some(TitlebarOptions { title: Some("Blazing Telesram".into()), ..Default::default() }),
            ..Default::default()
        },
        |window, cx| cx.new(|cx| Desktop::new(window_store, Clock::system(), window, cx)),
    );
    let handle = match opened {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("cannot open the chat window: {error}");
            return cx.quit();
        }
    };
    // Expiry and sign-out both lead back to the sign-in window.
    cx.subscribe(&store, move |store, event, cx| {
        if *event == StoreEvent::Session && store.read(cx).session() != SessionState::Active {
            open_sign_in(runtime.clone(), cx);
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    })
    .detach();
}

#[cfg(test)]
mod session_tests {
    use super::{LoginCookie, session_cookie};

    #[test]
    fn login_cookie_becomes_session_cookie_unchanged() {
        let cookie = LoginCookie {
            name: "Session_id".into(),
            value: "v".into(),
            domain: "yandex.ru".into(),
            path: "/".into(),
            expires_unix: Some(1_900_000_000),
            secure: true,
            http_only: true,
        };
        let converted = session_cookie(cookie);
        assert_eq!(
            (converted.name.as_str(), converted.value.as_str(), converted.domain.as_str(), converted.path.as_str()),
            ("Session_id", "v", "yandex.ru", "/")
        );
        assert_eq!((converted.expires_unix, converted.secure, converted.http_only), (Some(1_900_000_000), true, true));
        assert_eq!(yandex_proto::session::cookie_header(&[converted], "yandex.ru"), "Session_id=v");
    }
}
