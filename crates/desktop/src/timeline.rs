//! One timeline row: day separator, grouped bubble, attachments.

use crate::format::{avatar_color, clock_time, day_key, day_label, fit_thumbnail, human_size, Clock};
use crate::store::ChatStore;
use crate::theme::*;
use crate::widgets::avatar;
use bt_chat_core::model::{ChatId, ChatKind, MediaRef, Message, MessageBody, ReplyQuote};
use gpui::{prelude::*, *};

const GROUP_GAP_US: i64 = 5 * 60 * 1_000_000;
const BUBBLE_MAX: f32 = 480.;
const BUBBLE_PAD_X: f32 = 12.;
const THUMB_MAX: f32 = 320.;
const AVATAR: f32 = 36.;

/// Consecutive messages by one author within a few minutes of each other on the same day share a group.
pub fn grouped(earlier: &Message, later: &Message, clock: Clock) -> bool {
    earlier.author == later.author
        && earlier.outgoing == later.outgoing
        && later.timestamp - earlier.timestamp < GROUP_GAP_US
        && day_key(earlier.timestamp, clock) == day_key(later.timestamp, clock)
}

/// Renders message `ix` of `chat`; pure in the store so it can run inside list callbacks.
pub fn message_row(store: &ChatStore, chat: &ChatId, ix: usize, clock: Clock) -> AnyElement {
    let Some(messages) = store.timeline(chat).map(|t| &t.messages) else { return div().into_any_element() };
    let Some(m) = messages.get(ix) else { return div().into_any_element() };
    let prev = ix.checked_sub(1).map(|i| &messages[i]);
    let next = messages.get(ix + 1);
    let kind = store.chat(chat).map(|c| c.kind);

    let new_day = prev.is_none_or(|p| day_key(p.timestamp, clock) != day_key(m.timestamp, clock));
    let group_start = new_day || prev.is_none_or(|p| !grouped(p, m, clock));
    let group_end = next.is_none_or(|n| !grouped(m, n, clock));
    let in_group_chat = !m.outgoing && kind == Some(ChatKind::Group);
    let ts = m.timestamp;

    let avatar_slot = in_group_chat.then(|| {
        let name = m.author_name.as_deref().unwrap_or("?");
        if group_end {
            avatar(store, m.author_avatar.as_ref(), &m.author.0, name, AVATAR, format!("msg-{ts}-avatar"))
                .into_any_element()
        } else {
            div().size(px(AVATAR)).flex_none().into_any_element()
        }
    });

    div()
        .w_full()
        .flex()
        .flex_col()
        .when(new_day, |d| {
            d.child(
                div().w_full().flex().justify_center().pt(px(8.)).pb(px(4.)).child(
                    div()
                        .debug_selector(move || format!("msg-{ts}-day"))
                        .px(px(10.))
                        .py(px(2.))
                        .rounded_full()
                        .bg(rgb(PANEL))
                        .text_size(px(13.))
                        .text_color(rgb(MUTED))
                        .child(day_label(ts, clock)),
                ),
            )
        })
        .child(
            div()
                .w_full()
                .px(px(16.))
                .pt(px(if group_start { 6. } else { 2. }))
                .when(next.is_none(), |d| d.pb(px(10.)))
                .flex()
                .items_end()
                .gap(px(8.))
                .when(m.outgoing, |d| d.justify_end())
                .children(avatar_slot)
                .child(bubble(store, m, in_group_chat && group_start, clock)),
        )
        .into_any_element()
}

fn bubble(store: &ChatStore, m: &Message, show_name: bool, clock: Clock) -> Div {
    let ts = m.timestamp;
    div()
        .debug_selector(move || format!("msg-{ts}"))
        .max_w(px(BUBBLE_MAX))
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(3.))
        .px(px(BUBBLE_PAD_X))
        .py(px(7.))
        .rounded(px(12.))
        .bg(rgb(if m.outgoing { OUTGOING } else { INCOMING }))
        .when(show_name, |d| {
            d.child(
                div()
                    .debug_selector(move || format!("msg-{ts}-author"))
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(avatar_color(&m.author.0)))
                    .child(m.author_name.clone().unwrap_or_else(|| "Неизвестный".into())),
            )
        })
        .when_some(m.forwarded_from.as_ref(), |d, from| {
            d.child(
                div()
                    .debug_selector(move || format!("msg-{ts}-forwarded"))
                    .text_size(px(13.))
                    .text_color(rgb(ACCENT))
                    .child(format!("Переслано от {from}")),
            )
        })
        .when_some(m.reply_to.as_ref(), |d, quote| d.child(reply_quote(ts, quote)))
        .child(body(store, m))
        .child(
            div()
                .justify_end()
                .flex()
                .gap(px(4.))
                .text_size(px(11.))
                .text_color(rgb(MUTED))
                .when(m.edited, |d| {
                    d.child(div().debug_selector(move || format!("msg-{ts}-edited")).child("изм."))
                })
                .child(clock_time(ts, clock)),
        )
}

fn reply_quote(ts: i64, quote: &ReplyQuote) -> Div {
    div()
        .debug_selector(move || format!("msg-{ts}-reply"))
        .flex()
        .flex_col()
        .max_w(px(300.))
        .pl(px(8.))
        .border_l_2()
        .border_color(rgb(ACCENT))
        .when_some(quote.author.as_ref(), |d, author| {
            d.child(
                div()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(ACCENT))
                    .child(author.clone()),
            )
        })
        .child(div().truncate().text_size(px(13.)).text_color(rgb(MUTED)).child(quote.text.clone()))
}

fn body(store: &ChatStore, m: &Message) -> AnyElement {
    let ts = m.timestamp;
    match &m.body {
        MessageBody::Text(text) => text_block(text.clone()).into_any_element(),
        MessageBody::Image { media, width, height, caption } => div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(thumbnail(store, ts, media, *width, *height))
            .when_some(caption.as_ref(), |d, caption| d.child(text_block(caption.clone())))
            .into_any_element(),
        MessageBody::File { name, size, .. } => file_card(ts, name, *size).into_any_element(),
        MessageBody::Deleted => div()
            .debug_selector(move || format!("msg-{ts}-deleted"))
            .italic()
            .text_size(px(14.))
            .text_color(rgb(MUTED))
            .child("Сообщение удалено")
            .into_any_element(),
        MessageBody::Unsupported(kind) => div()
            .debug_selector(move || format!("msg-{ts}-unsupported"))
            .italic()
            .text_size(px(14.))
            .text_color(rgb(MUTED))
            .child(format!("Сообщение типа «{kind}» пока не поддерживается"))
            .into_any_element(),
    }
}

/// The bubble is content-sized, so gpui measures its text at max-content width and keeps that single-line
/// height even after the bubble is clamped; a max width on the text itself makes the measurement wrap.
fn text_block(text: String) -> Div {
    div().max_w(px(BUBBLE_MAX - 2. * BUBBLE_PAD_X)).text_size(px(15.)).line_height(px(21.)).child(text)
}

/// Fixed-size box at the image's aspect ratio, so the row height does not change when the file arrives.
fn thumbnail(store: &ChatStore, ts: i64, media: &MediaRef, width: u32, height: u32) -> Div {
    let (w, h) = fit_thumbnail(width, height, THUMB_MAX);
    div()
        .debug_selector(move || format!("msg-{ts}-image"))
        .relative()
        .w(px(w))
        .h(px(h))
        .flex_none()
        .rounded(px(8.))
        .bg(rgb(PLACEHOLDER))
        .when_some(store.media_path(media), |d, path| {
            d.child(
                img(path.to_path_buf())
                    .object_fit(ObjectFit::Cover)
                    .size_full()
                    .rounded(px(8.))
                    .debug_selector(move || format!("msg-{ts}-image-loaded")),
            )
        })
}

fn file_card(ts: i64, name: &str, size: u64) -> Div {
    let extension = name.rsplit_once('.').map_or("", |(_, ext)| ext);
    let badge: String = if extension.is_empty() || extension.chars().count() > 4 {
        "FILE".into()
    } else {
        extension.to_uppercase()
    };
    div()
        .debug_selector(move || format!("msg-{ts}-file"))
        .flex()
        .items_center()
        .gap(px(10.))
        .w(px(260.))
        .child(
            div()
                .size(px(44.))
                .flex_none()
                .rounded_full()
                .bg(rgb(BADGE))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.))
                .font_weight(FontWeight::BOLD)
                .child(badge),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().truncate().text_size(px(14.)).font_weight(FontWeight::MEDIUM).child(name.to_string()))
                .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(human_size(size))),
        )
}
