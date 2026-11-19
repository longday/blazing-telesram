//! Sign-in window: a GPUI window hosting an incognito wry WebView on the Telemost page.
//! The WebView is polled for the Yandex `Session_id` cookie; the caller verifies the cookies.
use gpui::{
    App, Bounds, Context, IntoElement, Render, Subscription, Task, TitlebarOptions, Window, WindowBounds, WindowHandle,
    WindowOptions, div, prelude::*, px, rgb, size,
};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    time::Duration,
};
use wry::{
    NewWindowResponse, Rect, WebView, WebViewBuilder,
    dpi::{LogicalPosition, LogicalSize},
};

const SIGN_IN_URL: &str = "https://telemost.yandex.ru/";
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const SESSION_COOKIE: &str = "Session_id";
const SESSION_DOMAIN: &str = "yandex.ru";

#[derive(Clone, PartialEq)]
pub struct LoginCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires_unix: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
}

/// Never prints the value: it is a credential.
impl std::fmt::Debug for LoginCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginCookie")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .field("domain", &self.domain)
            .field("path", &self.path)
            .field("expires_unix", &self.expires_unix)
            .field("secure", &self.secure)
            .field("http_only", &self.http_only)
            .finish()
    }
}

/// Missing domain becomes empty (WebKit always reports one), missing path `/`;
/// session cookies and unrepresentable dates have no expiry.
pub fn convert_cookie(c: &cookie::Cookie<'_>) -> LoginCookie {
    LoginCookie {
        name: c.name().to_owned(),
        value: c.value().to_owned(),
        domain: c.domain().unwrap_or_default().to_owned(),
        path: c.path().unwrap_or("/").to_owned(),
        expires_unix: match c.expires() {
            Some(cookie::Expiration::DateTime(at)) => Some(at.unix_timestamp()),
            _ => None,
        },
        secure: c.secure().unwrap_or(false),
        http_only: c.http_only().unwrap_or(false),
    }
}

fn has_session(cookies: &[LoginCookie]) -> bool {
    cookies.iter().any(|c| {
        let domain = c.domain.trim_start_matches('.');
        c.name == SESSION_COOKIE
            && !c.value.is_empty()
            && (domain == SESSION_DOMAIN || domain.ends_with(".yandex.ru"))
    })
}

/// Scheme and host only: paths and queries of sign-in URLs may carry tokens.
fn origin(url: &str) -> &str {
    let after_scheme = url.find("://").map_or(0, |i| i + 3);
    let end = url[after_scheme..].find(['/', '?', '#']).map_or(url.len(), |i| after_scheme + i);
    &url[..end]
}

pub struct SignInWindow {
    webview: Option<Rc<WebView>>,
    paused: Rc<Cell<bool>>,
    _poll: Task<()>,
    _bounds: Subscription,
}

impl SignInWindow {
    /// Call after a failed verification: polling stays paused while `on_cookies` is being handled.
    pub fn resume_polling(&mut self) {
        self.paused.set(false);
    }

    fn fit_webview(&self, window: &Window) {
        let Some(webview) = &self.webview else { return };
        let view = window.viewport_size();
        let bounds = Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: LogicalSize::new(f64::from(f32::from(view.width)), f64::from(f32::from(view.height))).into(),
        };
        if let Err(error) = webview.set_bounds(bounds) {
            eprintln!("sign-in webview resize failed: {error}");
        }
    }
}

impl Render for SignInWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // The WebView is a native child view drawn above this background.
        div().size_full().bg(rgb(0x111c2a))
    }
}

/// Must run on the main thread (wry requirement). `on_cookies` gets every cookie of the incognito
/// store once `Session_id` shows up and polling pauses until `resume_polling`.
pub fn open_sign_in_window(
    cx: &mut App,
    on_cookies: impl Fn(Vec<LoginCookie>, &mut Window, &mut App) + 'static,
) -> Result<WindowHandle<SignInWindow>, String> {
    let bounds = Bounds::centered(None, size(px(480.), px(720.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some("Вход в Яндекс".into()), ..Default::default() }),
        ..Default::default()
    };
    let failure = Rc::new(Cell::new(None::<String>));
    let build_failure = failure.clone();
    let handle = cx
        .open_window(options, move |window, cx| {
            let paused = Rc::new(Cell::new(false));
            // Weak: the handler lives inside the WebView it points at.
            let this_view = Rc::new(RefCell::new(Weak::<WebView>::new()));
            let popup_target = this_view.clone();
            let webview = WebViewBuilder::new()
                .with_incognito(true)
                .with_url(SIGN_IN_URL)
                // Without a handler WebKit silently drops `window.open`/`target=_blank`, and the
                // Telemost "Войти" button opens Passport that way. Cookies are polled, so the
                // whole flow can run in this one view.
                .with_new_window_req_handler(move |url, _| {
                    eprintln!("sign-in: opening {} in the sign-in view", origin(&url));
                    if let Some(view) = popup_target.borrow().upgrade()
                        && let Err(error) = view.load_url(&url)
                    {
                        eprintln!("sign-in: cannot open the page: {error}");
                    }
                    NewWindowResponse::Deny
                })
                .build_as_child(&*window)
                .map(Rc::new);
            let webview = match webview {
                Ok(webview) => {
                    *this_view.borrow_mut() = Rc::downgrade(&webview);
                    Some(webview)
                }
                Err(error) => {
                    build_failure.set(Some(error.to_string()));
                    None
                }
            };
            let poll = match &webview {
                Some(webview) => {
                    let (webview, paused) = (webview.clone(), paused.clone());
                    window.spawn(cx, async move |cx| {
                        loop {
                            cx.background_executor().timer(POLL_INTERVAL).await;
                            if paused.get() {
                                continue;
                            }
                            let Ok(cookies) = webview.cookies() else { continue };
                            let cookies: Vec<LoginCookie> = cookies.iter().map(convert_cookie).collect();
                            if !has_session(&cookies) {
                                continue;
                            }
                            paused.set(true);
                            if cx.update(|window, cx| on_cookies(cookies, window, cx)).is_err() {
                                return;
                            }
                        }
                    })
                }
                None => Task::ready(()),
            };
            cx.new(|cx| {
                let subscription =
                    cx.observe_window_bounds(window, |this: &mut SignInWindow, window, _| this.fit_webview(window));
                let this = SignInWindow { webview, paused, _poll: poll, _bounds: subscription };
                this.fit_webview(window);
                if let Some(webview) = &this.webview {
                    let _ = webview.focus();
                }
                this
            })
        })
        .map_err(|error| error.to_string())?;
    if let Some(error) = failure.take() {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        return Err(error);
    }
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cookie::{Cookie, Expiration, time::OffsetDateTime};

    #[test]
    fn converts_all_attributes() {
        let at = OffsetDateTime::from_unix_timestamp(1_900_000_000).unwrap();
        let c = Cookie::build(("Session_id", "v"))
            .domain(".yandex.ru")
            .path("/x")
            .expires(Expiration::DateTime(at))
            .secure(true)
            .http_only(true)
            .build();
        assert_eq!(
            convert_cookie(&c),
            LoginCookie {
                name: "Session_id".into(),
                value: "v".into(),
                // The cookie crate strips the leading dot of a domain attribute.
                domain: "yandex.ru".into(),
                path: "/x".into(),
                expires_unix: Some(1_900_000_000),
                secure: true,
                http_only: true,
            }
        );
    }

    #[test]
    fn missing_attributes_get_defaults() {
        let c = convert_cookie(&Cookie::new("a", "b"));
        assert_eq!(c.domain, "");
        assert_eq!(c.path, "/");
        assert_eq!(c.expires_unix, None);
        assert!(!c.secure && !c.http_only);
    }

    #[test]
    fn session_expiry_has_no_timestamp() {
        let c = Cookie::build(("a", "b")).expires(Expiration::Session).build();
        assert_eq!(convert_cookie(&c).expires_unix, None);
    }

    fn cookie(name: &str, value: &str, domain: &str) -> LoginCookie {
        convert_cookie(&Cookie::build((name.to_owned(), value.to_owned())).domain(domain.to_owned()).build())
    }

    #[test]
    fn detects_non_empty_session_on_yandex_domain() {
        assert!(has_session(&[cookie("Session_id", "x", ".yandex.ru")]));
        assert!(has_session(&[cookie("Session_id", "x", "yandex.ru")]));
        assert!(!has_session(&[cookie("Session_id", "", ".yandex.ru")]));
        assert!(!has_session(&[cookie("sessionid2", "x", ".yandex.ru")]));
        assert!(!has_session(&[cookie("Session_id", "x", ".example.com")]));
        assert!(!has_session(&[cookie("Session_id", "x", ".notyandex.ru")]));
    }

    #[test]
    fn origin_drops_path_and_query() {
        assert_eq!(origin("https://passport.yandex.ru/auth?retpath=x&token=t"), "https://passport.yandex.ru");
        assert_eq!(origin("https://sso.yandex.ru#frag"), "https://sso.yandex.ru");
        assert_eq!(origin("about:blank"), "about:blank");
    }
}
