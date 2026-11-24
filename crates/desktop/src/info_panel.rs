//! Chat info panel: the people of the open chat, read-only, in a column right of the chat pane.
//!
//! What it shows is a pure function of the store's `MembersState` (`body`); the rows are built on demand
//! for the visible range, because a list may hold a thousand people. Only users are listed: the server's
//! `groups` and `departments` entries have no modelled shape (design D6) and the panel never invents them.

use crate::desktop::Desktop;
use crate::store::{ChatStore, MembersState};
use crate::theme::*;
use crate::widgets::{avatar, button};
use bt_chat_core::model::{ChatId, ChatKind, Member, MemberRole};
use gpui::{prelude::*, *};
use std::ops::Range;

pub const WIDTH: f32 = 320.;
const ROW_HEIGHT: f32 = 52.;

/// What the panel shows below its header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// The request is on its way, or has not been made yet: never an empty list.
    Loading,
    /// The client was offline when the panel was opened.
    Offline,
    /// The request failed; the text is the reason (for a refused list it says the list is not available
    /// to this account).
    Failed(String),
    /// `shown` people are listed; `partial` is set when the paging bound was reached and more may exist.
    Ready { shown: usize, partial: bool },
}

pub fn body(state: Option<&MembersState>) -> Body {
    match state {
        None | Some(MembersState::Loading) => Body::Loading,
        Some(MembersState::Failed { offline: true, .. }) => Body::Offline,
        Some(MembersState::Failed { reason, .. }) => Body::Failed(reason.clone()),
        Some(MembersState::Ready(members)) => Body::Ready { shown: members.users.len(), partial: members.partial },
    }
}

/// The line over the list: the count, or, for a bounded list, how many are shown and that more may exist.
pub fn count_text(shown: usize, partial: bool) -> String {
    if partial {
        format!("Показано участников: {shown}. Список может быть неполным")
    } else {
        format!("Участников: {shown}")
    }
}

fn notice(selector: &'static str, text: impl Into<SharedString>) -> Div {
    div().debug_selector(move || selector.into()).px(px(16.)).py(px(10.)).text_color(rgb(MUTED)).child(text.into())
}

fn retry(cx: &mut Context<Desktop>) -> Stateful<Div> {
    button("info-retry", "Повторить")
        .debug_selector(|| "info-retry".into())
        .on_click(cx.listener(|this, _, _, cx| this.reload_info(cx)))
}

/// One person: avatar (or initials), name, the administrator marker for the admin role only and "Написать"
/// (which starts the private chat flow) on every row but the current user's.
/// Debug selectors: `member-<guid>`, `member-<guid>-avatar`, `member-<guid>-admin`, `member-<guid>-write`.
fn member_row(store: &ChatStore, member: &Member, cx: &Context<Desktop>) -> Div {
    let tag = format!("member-{}", member.user.0);
    let name = member.name.clone().unwrap_or_else(|| "Без имени".into());
    let admin = member.role == MemberRole::Admin;
    let admin_tag = format!("{tag}-admin");
    let write_tag = format!("{tag}-write");
    let row_tag = tag.clone();
    let user = member.user.clone();
    div()
        .debug_selector(move || row_tag)
        .h(px(ROW_HEIGHT))
        .w_full()
        .px(px(16.))
        .flex()
        .items_center()
        .gap(px(12.))
        .child(avatar(store, member.avatar.as_ref(), &member.user.0, &name, 36., format!("{tag}-avatar")))
        .child(div().flex_1().min_w_0().truncate().child(name))
        .when(admin, |row| {
            row.child(
                div()
                    .debug_selector(move || admin_tag)
                    .flex_none()
                    .px(px(8.))
                    .py(px(2.))
                    .rounded_full()
                    .bg(rgb(FIELD))
                    .text_size(px(12.))
                    .text_color(rgb(ACCENT))
                    .child("админ"),
            )
        })
        .when(!member.me, |row| {
            row.child(
                button(SharedString::from(write_tag.clone()), "Написать")
                    .debug_selector(move || write_tag)
                    .flex_none()
                    .px(px(8.))
                    .py(px(3.))
                    .text_size(px(12.))
                    .text_color(rgb(ACCENT))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_new_chat(Some(user.clone()), cx))),
            )
        })
}

/// The rows of `range` of the loaded list of `chat`; nothing while there is no list.
fn rows(store: &ChatStore, chat: &ChatId, range: Range<usize>, cx: &Context<Desktop>) -> Vec<Div> {
    let Some(MembersState::Ready(members)) = store.members_state(chat) else { return Vec::new() };
    range.filter_map(|ix| members.users.get(ix)).map(|member| member_row(store, member, cx)).collect()
}

/// The panel of `chat`. Debug selectors: `info-panel`, `info-close`, `info-loading`, `info-offline`,
/// `info-error`, `info-error-reason`, `info-retry`, `info-count`, `info-partial`, `info-empty`, `info-members`.
pub fn render(chat: &ChatId, store: &Entity<ChatStore>, cx: &mut Context<Desktop>) -> Div {
    let (title, kind, body) = {
        let state = store.read(cx);
        let summary = state.chat(chat);
        let title = summary.map_or_else(|| "Чат".to_string(), |c| c.title.clone());
        let kind = summary.map_or("", |c| match c.kind {
            ChatKind::Private => "Личный чат",
            ChatKind::Group => "Группа",
            ChatKind::Channel => "Канал",
        });
        (title, kind, body(state.members_state(chat)))
    };
    let content = match body {
        Body::Loading => notice("info-loading", "Загрузка участников…").into_any_element(),
        Body::Offline => div()
            .flex()
            .flex_col()
            .items_start()
            .child(notice("info-offline", "Список участников недоступен без сети"))
            .child(div().px(px(16.)).child(retry(cx)))
            .into_any_element(),
        Body::Failed(reason) => div()
            .flex()
            .flex_col()
            .items_start()
            .child(notice("info-error", "Не удалось загрузить участников"))
            .child(
                div()
                    .debug_selector(|| "info-error-reason".into())
                    .px(px(16.))
                    .pb(px(10.))
                    .text_size(px(13.))
                    .text_color(rgb(MUTED))
                    .child(reason),
            )
            .child(div().px(px(16.)).child(retry(cx)))
            .into_any_element(),
        Body::Ready { shown, partial } => {
            let chat = chat.clone();
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    notice(if partial { "info-partial" } else { "info-count" }, count_text(shown, partial))
                        .flex_none()
                        .text_size(px(13.)),
                )
                .when(shown == 0, |d| d.child(notice("info-empty", "Нет данных об участниках")))
                .when(shown > 0, |d| {
                    d.child(
                        div().debug_selector(|| "info-members".into()).flex_1().min_h_0().child(
                            uniform_list(
                                "info-members",
                                shown,
                                cx.processor(move |this, range: Range<usize>, _, cx| rows(this.store.read(cx), &chat, range, cx)),
                            )
                            .size_full(),
                        ),
                    )
                })
                .into_any_element()
        }
    };
    div()
        .debug_selector(|| "info-panel".into())
        .w(px(WIDTH))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .bg(rgb(PANEL))
        .border_l_1()
        .border_color(rgb(DIVIDER))
        .child(
            div()
                .h(px(56.))
                .flex_none()
                .px(px(16.))
                .flex()
                .items_center()
                .gap(px(8.))
                .border_b_1()
                .border_color(rgb(DIVIDER))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().truncate().font_weight(FontWeight::MEDIUM).child(title))
                        .child(div().text_size(px(13.)).text_color(rgb(MUTED)).child(kind)),
                )
                .child(
                    div()
                        .id("info-close")
                        .debug_selector(|| "info-close".into())
                        .size(px(32.))
                        .flex_none()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(MUTED))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(ROW_HOVER)))
                        .child("✕")
                        .on_click(cx.listener(|this, _, _, cx| this.close_info(cx))),
                ),
        )
        .child(content)
}
