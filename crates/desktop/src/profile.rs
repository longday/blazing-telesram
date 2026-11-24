//! The account's own profile, read-only, as a modal over the chat.
//!
//! What it shows is a pure function of the store's `ProfileState` (`body`). There is no control that
//! changes the name, the avatar or the status: the view is text, an avatar and two buttons (retry, close).
//! A request always starts from `Loading`, so nothing from an earlier request or session is shown as current.

use crate::desktop::Desktop;
use crate::store::{ChatStore, ProfileState};
use crate::theme::*;
use crate::widgets::{avatar, button};
use gpui::{prelude::*, *};

/// What the view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// The request is on its way, or has not been made yet.
    Loading,
    /// The client was offline when the profile was opened.
    Offline,
    /// The request failed; the text is the reason.
    Failed(String),
    /// The account as the server reports it; the status is set only when the server sent a non-blank text.
    Ready { name: String, status: Option<String> },
}

pub fn body(state: Option<&ProfileState>) -> Body {
    match state {
        None | Some(ProfileState::Loading) => Body::Loading,
        Some(ProfileState::Failed { offline: true, .. }) => Body::Offline,
        Some(ProfileState::Failed { reason, .. }) => Body::Failed(reason.clone()),
        Some(ProfileState::Ready(profile)) => Body::Ready {
            name: profile.name.clone().unwrap_or_else(|| "Без имени".into()),
            status: profile.status_text.clone(),
        },
    }
}

fn notice(selector: &'static str, text: impl Into<SharedString>) -> Div {
    div().debug_selector(move || selector.into()).text_color(rgb(MUTED)).child(text.into())
}

fn retry(cx: &mut Context<Desktop>) -> Stateful<Div> {
    button("profile-retry", "Повторить")
        .debug_selector(|| "profile-retry".into())
        .on_click(cx.listener(|this, _, _, cx| this.reload_profile(cx)))
}

/// The modal. Debug selectors: `profile-view`, `profile-close`, `profile-loading`, `profile-offline`,
/// `profile-error`, `profile-error-reason`, `profile-retry`, `profile-avatar`, `profile-name`, `profile-status`.
pub fn render(store: &Entity<ChatStore>, cx: &mut Context<Desktop>) -> Div {
    let content = {
        let state = store.read(cx);
        match body(state.profile_state()) {
            Body::Loading => notice("profile-loading", "Загрузка профиля…").into_any_element(),
            Body::Offline => div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.))
                .child(notice("profile-offline", "Профиль недоступен без сети"))
                .child(retry(cx))
                .into_any_element(),
            Body::Failed(reason) => div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.))
                .child(notice("profile-error", "Не удалось загрузить профиль"))
                .child(
                    div()
                        .debug_selector(|| "profile-error-reason".into())
                        .text_size(px(13.))
                        .text_color(rgb(MUTED))
                        .child(reason),
                )
                .child(retry(cx))
                .into_any_element(),
            Body::Ready { name, status } => {
                let profile = match state.profile_state() {
                    Some(ProfileState::Ready(profile)) => Some(profile),
                    _ => None,
                };
                let (key, media) = profile.map_or(("", None), |p| (p.user.0.as_str(), p.avatar.as_ref()));
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.))
                    .child(avatar(state, media, key, &name, 96., "profile-avatar".into()))
                    .child(
                        div()
                            .debug_selector(|| "profile-name".into())
                            .text_size(px(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(name),
                    )
                    .when_some(status, |d, status| {
                        d.child(div().debug_selector(|| "profile-status".into()).text_color(rgb(MUTED)).child(status))
                    })
                    .into_any_element()
            }
        }
    };
    div()
        .debug_selector(|| "profile-view".into())
        .absolute()
        .size_full()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x000000aa))
        .child(
            div()
                .w(px(380.))
                .p(px(20.))
                .rounded(px(12.))
                .bg(rgb(PANEL))
                .flex()
                .flex_col()
                .gap(px(16.))
                .child(div().text_size(px(17.)).font_weight(FontWeight::MEDIUM).child("Профиль"))
                .child(div().flex().justify_center().text_center().child(content))
                .child(
                    div().flex().justify_end().child(
                        button("profile-close", "Закрыть")
                            .debug_selector(|| "profile-close".into())
                            .on_click(cx.listener(|this, _, _, cx| this.close_profile(cx))),
                    ),
                ),
        )
}
