mod desktop;
mod format;
mod input;
mod login;
mod logging;
mod store;
mod theme;
mod timeline;
mod widgets;
mod yandex_backend;
#[cfg(test)]
mod tests;

use bt_chat_core::model::UserId;
use bt_chat_core::{ChatClient, ClientConfig};
use desktop::{Desktop, Quit, Search, ToggleLog};
use format::Clock;
use gpui::{prelude::*, *};
use login::{LoginCookie, SignInWindow};
use store::{ChatStore, SessionState, StoreEvent};
use yandex_backend::YandexBackend;
use yandex_proto::bootstrap::{BootstrapError, RegistryEndpoints, bootstrap};
use yandex_proto::runtime::RuntimeDir;
use yandex_proto::session::{Identity, Session, SessionCookie};

fn main() {
    // Nothing can be logged before the runtime directory and filter are known.
    let runtime = RuntimeDir::from_env().map_err(|e| e.to_string());
    let (runtime, log) = match runtime.and_then(|runtime| logging::init(runtime.path()).map(|log| (runtime, log))) {
        Ok(started) => started,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting");
    Application::new().run(move |cx: &mut App| {
        cx.set_global(log);
        input::bind(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-f", Search, Some("BlazingTelesram")),
            KeyBinding::new("ctrl-f", Search, Some("BlazingTelesram")),
            KeyBinding::new("cmd-shift-l", ToggleLog, Some("BlazingTelesram")),
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
                tracing::error!(%error, "cannot read the stored session");
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
                        tracing::info!("signed in");
                        // Open the next window first: closing the last one quits the app.
                        open_chat_window(runtime, session, cx);
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    }
                    Err(error) => {
                        tracing::warn!(%error, "sign-in verification failed");
                        let _ = handle.update(cx, |view, _, _| view.resume_polling());
                    }
                }
            });
        })
        .detach();
    });
    if let Err(error) = opened {
        tracing::error!(%error, "cannot open the sign-in window");
        cx.quit();
    }
}

fn open_chat_window(runtime: RuntimeDir, session: Session, cx: &mut App) {
    let account = UserId(session.identity.xiva_user().to_owned());
    let config = ClientConfig::new(runtime.path(), runtime.session_file(), account);
    let client = match ChatClient::spawn(YandexBackend::new(session), config) {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(%error, "cannot start the chat client");
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
    let bounds = fitted_window_bounds(size(px(1180.), px(800.)), cx);
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
            tracing::error!(%error, "cannot open the chat window");
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

/// Centers `preferred` in the primary screen area left by the menu bar and Dock, shrunk so that the window,
/// title bar included, fits it.
pub(crate) fn fitted_window_bounds(preferred: Size<Pixels>, cx: &App) -> Bounds<Pixels> {
    use objc2_app_kit::{NSScreen, NSWindow, NSWindowStyleMask};
    let Some(mtm) = objc2::MainThreadMarker::new() else { return Bounds::centered(None, preferred, cx) };
    let (Some(screen), Some(primary)) = (NSScreen::mainScreen(mtm), cx.primary_display()) else {
        return Bounds::centered(None, preferred, cx);
    };
    let visible = screen.visibleFrame();
    let content = NSWindow::contentRectForFrameRect_styleMask(visible, NSWindowStyleMask::Titled, mtm);
    let fitted = preferred.min(&size(px(content.size.width as f32), px(content.size.height as f32)));
    // gpui takes the frame's top-left corner with the content size. AppKit measures y up from the primary
    // screen's bottom edge; gpui measures it down from its top edge.
    let top = primary.bounds().size.height - px((visible.origin.y + visible.size.height) as f32);
    let frame_height = fitted.height + px((visible.size.height - content.size.height) as f32);
    let origin = point(
        px(visible.origin.x as f32) + (px(visible.size.width as f32) - fitted.width) / 2.,
        top + (px(visible.size.height as f32) - frame_height) / 2.,
    );
    Bounds::new(origin, fitted)
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
