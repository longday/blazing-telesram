//! One timeline row: day separator, grouped bubble, attachments, reaction chips.

use crate::desktop::Desktop;
use crate::format::{
    Clock, avatar_color, clock_time, day_key, day_label, fit_thumbnail, human_size,
};
use crate::store::ChatStore;
use crate::theme::*;
use crate::viewer::{Action, ActionLine, action_line};
use crate::widgets::avatar;
use bt_chat_core::model::{
    ChatId, ChatKind, ConnectionState, Delivery, GalleryItem, MediaRef, Message, MessageBody,
    MessageTs, ReplyQuote,
};
use bt_chat_core::reactions::{NEUTRAL_GLYPH, ReactionCount, emoji};
use gpui::{prelude::*, *};
use std::cell::RefCell;
use std::rc::Rc;

const GROUP_GAP_US: i64 = 5 * 60 * 1_000_000;
const BUBBLE_MAX: f32 = 480.;
const BUBBLE_PAD_X: f32 = 12.;
const THUMB_MAX: f32 = 320.;
const AVATAR: f32 = 36.;
/// Width available to the content of a bubble.
const CONTENT_W: f32 = BUBBLE_MAX - 2. * BUBBLE_PAD_X;
const GRID_GAP: f32 = 4.;

/// Consecutive messages by one author within a few minutes of each other on the same day share a group.
pub fn grouped(earlier: &Message, later: &Message, clock: Clock) -> bool {
    earlier.author == later.author
        && earlier.outgoing == later.outgoing
        && later.timestamp - earlier.timestamp < GROUP_GAP_US
        && day_key(earlier.timestamp, clock) == day_key(later.timestamp, clock)
}

/// Where each row that was painted in the latest frame sits, in window coordinates. The list reports
/// no item bounds while it follows the bottom, so the rows report them themselves; whoever reads the
/// frame drains it.
pub type RowBounds = Rc<RefCell<Vec<(MessageTs, Bounds<Pixels>)>>>;

/// What a row needs besides the store's data: the entities its clicks act on.
#[derive(Clone)]
pub struct RowContext {
    pub store: Entity<ChatStore>,
    pub desktop: WeakEntity<Desktop>,
    pub chat: ChatId,
    pub clock: Clock,
    pub rows: RowBounds,
}

/// Renders message `ix` of the context's chat; pure in the store so it can run inside list callbacks.
pub fn message_row(ctx: &RowContext, ix: usize, cx: &App) -> AnyElement {
    let (store, chat, clock) = (ctx.store.read(cx), &ctx.chat, ctx.clock);
    let Some(messages) = store.timeline(chat).map(|t| &t.messages) else {
        return div().into_any_element();
    };
    let Some(m) = messages.get(ix) else {
        return div().into_any_element();
    };
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
            avatar(
                store,
                m.author_avatar.as_ref(),
                &m.author.0,
                name,
                AVATAR,
                format!("msg-{ts}-avatar"),
            )
            .into_any_element()
        } else {
            div().size(px(AVATAR)).flex_none().into_any_element()
        }
    });

    div()
        .w_full()
        .flex()
        .flex_col()
        .on_children_prepainted({
            let rows = ctx.rows.clone();
            move |children, _, _| {
                if let (Some(first), Some(last)) = (children.first(), children.last()) {
                    rows.borrow_mut()
                        .push((ts, Bounds::from_corners(first.origin, last.bottom_right())));
                }
            }
        })
        .when(new_day, |d| {
            d.child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .pt(px(8.))
                    .pb(px(4.))
                    .child(
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
                .child(bubble(ctx, store, m, in_group_chat && group_start)),
        )
        .into_any_element()
}

fn bubble(ctx: &RowContext, store: &ChatStore, m: &Message, show_name: bool) -> Div {
    let ts = m.timestamp;
    let (desktop, chat) = (ctx.desktop.clone(), ctx.chat.clone());
    div()
        .on_mouse_down(MouseButton::Right, move |event, _, cx| {
            desktop
                .update(cx, |desktop, cx| {
                    desktop.open_menu(chat.clone(), ts, event.position, cx)
                })
                .ok();
        })
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
                    .child(
                        m.author_name
                            .clone()
                            .unwrap_or_else(|| "Неизвестный".into()),
                    ),
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
        .when_some(m.reply_to.as_ref(), |d, quote| {
            d.child(reply_quote(ts, quote))
        })
        .child(body(ctx, store, m))
        .when_some(failure(ctx, m), |d, failure| d.child(failure))
        .when_some(reaction_chips(ctx, m), |d, chips| d.child(chips))
        .child(
            div()
                .justify_end()
                .items_center()
                .flex()
                .gap(px(4.))
                .text_size(px(11.))
                .text_color(rgb(MUTED))
                .when(m.edited, |d| {
                    d.child(
                        div()
                            .debug_selector(move || format!("msg-{ts}-edited"))
                            .child("изм."),
                    )
                })
                .child(clock_time(ts, ctx.clock))
                .when(m.outgoing, |d| d.child(delivery_mark(ts, &m.delivery))),
        )
}

/// What the sender sees at the message time: a clock while queued, a check once the server has it, a red "!" when refused.
fn delivery_mark(ts: i64, delivery: &Delivery) -> Div {
    let (selector, glyph, color) = match delivery {
        Delivery::Pending(_) => ("pending", "🕓", MUTED),
        Delivery::Sent => ("sent", "✓", SENT_MARK),
        Delivery::Failed { .. } => ("failed", "!", DANGER),
    };
    div()
        .debug_selector(move || format!("msg-{ts}-{selector}"))
        .text_color(rgb(color))
        .child(glyph)
}

/// The refusal reason and the retry action of a failed message.
fn failure(ctx: &RowContext, m: &Message) -> Option<Div> {
    let Delivery::Failed { payload, reason } = &m.delivery else {
        return None;
    };
    let ts = m.timestamp;
    let (store, payload) = (ctx.store.clone(), payload.clone());
    Some(
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(13.))
            .child(
                div()
                    .debug_selector(move || format!("msg-{ts}-failed-reason"))
                    .text_color(rgb(DANGER))
                    .child(format!("Не отправлено: {reason}")),
            )
            .child(
                div()
                    .id(SharedString::from(format!("retry-{ts}")))
                    .debug_selector(move || format!("msg-{ts}-retry"))
                    .text_color(rgb(ACCENT))
                    .cursor_pointer()
                    .child("Повторить")
                    .on_click(move |_, _, cx| {
                        store.update(cx, |store, _| store.retry(payload.clone()))
                    }),
            ),
    )
}

/// The reaction chips of a sent, non-deleted message: a wrapping row below the body. A chip shows the
/// type's emoji (the neutral glyph for a code outside the table) and its count, the account's own
/// type is highlighted, and a left click toggles that type. Debug selectors: `msg-{ts}-reaction-{code}`.
fn reaction_chips(ctx: &RowContext, m: &Message) -> Option<Div> {
    if !matches!(m.delivery, Delivery::Sent)
        || matches!(m.body, MessageBody::Deleted)
        || m.reactions.counts.is_empty()
    {
        return None;
    }
    let ts = m.timestamp;
    let chips = m
        .reactions
        .counts
        .iter()
        .map(|&ReactionCount { code, count }| {
            let own = m.reactions.mine == Some(code);
            let (desktop, chat) = (ctx.desktop.clone(), ctx.chat.clone());
            div()
                .id(SharedString::from(format!("reaction-{ts}-{}", code.0)))
                .debug_selector(move || format!("msg-{ts}-reaction-{}", code.0))
                .flex()
                .items_center()
                .gap(px(4.))
                .px(px(8.))
                .py(px(2.))
                .rounded_full()
                .bg(rgb(if own { REACTION_OWN } else { REACTION_CHIP }))
                .cursor_pointer()
                .child(
                    div()
                        .text_size(px(14.))
                        .child(emoji(code).unwrap_or(NEUTRAL_GLYPH)),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(INK))
                        .child(count.to_string()),
                )
                .on_click(move |_, _, cx| {
                    desktop
                        .update(cx, |desktop, cx| {
                            desktop.toggle_reaction(chat.clone(), ts, code, cx)
                        })
                        .ok();
                })
        });
    Some(
        div()
            .flex()
            .flex_wrap()
            .gap(px(4.))
            .pt(px(2.))
            .children(chips),
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
        .child(
            div()
                .truncate()
                .text_size(px(13.))
                .text_color(rgb(MUTED))
                .child(quote.text.clone()),
        )
}

fn body(ctx: &RowContext, store: &ChatStore, m: &Message) -> AnyElement {
    let ts = m.timestamp;
    match &m.body {
        MessageBody::Text(text) => text_block(text.clone()).into_any_element(),
        MessageBody::Image {
            media,
            width,
            height,
            caption,
            ..
        } => attachment(
            ts,
            Some(single_image(ctx, store, ts, media, *width, *height)),
            caption.as_deref(),
        ),
        MessageBody::Gallery { items, caption } => {
            let pictures = match items.as_slice() {
                [] => None,
                [one] => Some(single_image(
                    ctx, store, ts, &one.media, one.width, one.height,
                )),
                many => Some(grid(ctx, store, ts, many)),
            };
            attachment(ts, pictures, caption.as_deref())
        }
        MessageBody::File { media, name, size } => {
            file_card(ctx, store, ts, media, name, *size).into_any_element()
        }
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

/// Pictures with their caption (`msg-{ts}-caption`) below.
fn attachment(ts: i64, pictures: Option<AnyElement>, caption: Option<&str>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .children(pictures)
        .when_some(caption, |d, caption| {
            d.child(
                text_block(caption.to_owned()).debug_selector(move || format!("msg-{ts}-caption")),
            )
        })
        .into_any_element()
}

/// The bubble is content-sized, so gpui measures its text at max-content width and keeps that single-line
/// height even after the bubble is clamped; a max width on the text itself makes the measurement wrap.
fn text_block(text: String) -> Div {
    div()
        .max_w(px(CONTENT_W))
        .text_size(px(15.))
        .line_height(px(21.))
        .child(text)
}

/// Columns of the picture grid: one picture on its own, two or four in two columns, otherwise three.
pub fn gallery_columns(n: usize) -> usize {
    match n {
        0 | 1 => 1,
        2 | 4 => 2,
        _ => 3,
    }
}

/// Side of a square grid cell: the columns and their gaps fill the bubble's content width.
fn cell_side(columns: usize) -> f32 {
    (CONTENT_W - GRID_GAP * (columns - 1) as f32) / columns as f32
}

/// Square cells in reading order, rows of `gallery_columns(len)`, the last row left-aligned. Every
/// cell has its size from the start, so the row height does not change when a preview arrives.
fn grid(ctx: &RowContext, store: &ChatStore, ts: i64, items: &[GalleryItem]) -> AnyElement {
    let columns = gallery_columns(items.len());
    let side = cell_side(columns);
    div()
        .flex()
        .flex_col()
        .gap(px(GRID_GAP))
        .children(items.chunks(columns).enumerate().map(|(row, cells)| {
            div()
                .flex()
                .gap(px(GRID_GAP))
                .children(cells.iter().enumerate().map(|(column, item)| {
                    let index = row * columns + column;
                    picture(
                        ctx,
                        store,
                        ts,
                        index,
                        &item.media,
                        side,
                        side,
                        format!("image-{index}"),
                    )
                }))
        }))
        .into_any_element()
}

/// One picture at its aspect ratio.
fn single_image(
    ctx: &RowContext,
    store: &ChatStore,
    ts: i64,
    media: &MediaRef,
    width: u32,
    height: u32,
) -> AnyElement {
    let (w, h) = fit_thumbnail(width, height, THUMB_MAX);
    picture(ctx, store, ts, 0, media, w, h, "image".into()).into_any_element()
}

/// Fixed-size box with the placeholder colour until the preview arrives, cropped to fill; a click opens
/// the viewer at picture `index`. Debug selectors: `msg-{ts}-{tag}`, plus `-loaded` once the preview shows.
#[allow(clippy::too_many_arguments)]
fn picture(
    ctx: &RowContext,
    store: &ChatStore,
    ts: i64,
    index: usize,
    media: &MediaRef,
    w: f32,
    h: f32,
    tag: String,
) -> Stateful<Div> {
    let (desktop, chat) = (ctx.desktop.clone(), ctx.chat.clone());
    let loaded = format!("msg-{ts}-{tag}-loaded");
    div()
        .id(SharedString::from(format!("picture-{ts}-{index}")))
        .debug_selector(move || format!("msg-{ts}-{tag}"))
        .relative()
        .w(px(w))
        .h(px(h))
        .flex_none()
        .overflow_hidden()
        .rounded(px(8.))
        .bg(rgb(PLACEHOLDER))
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            desktop
                .update(cx, |desktop, cx| {
                    desktop.open_viewer(chat.clone(), ts, index, cx)
                })
                .ok();
        })
        .when_some(store.media_path(media), |d, path| {
            d.child(
                img(path.to_path_buf())
                    .object_fit(ObjectFit::Cover)
                    .size_full()
                    .rounded(px(8.))
                    .debug_selector(move || loaded),
            )
        })
}

/// The file's name, size and one action line that follows its download state (design Decision 7).
/// Debug selectors: `msg-{ts}-file`, and in the action line `-download`, `-cancel`, `-retry` for the
/// button, `-saved` for the "saved as" text and `-status` for any other text.
fn file_card(
    ctx: &RowContext,
    store: &ChatStore,
    ts: i64,
    media: &MediaRef,
    name: &str,
    size: u64,
) -> Div {
    let extension = name.rsplit_once('.').map_or("", |(_, ext)| ext);
    let badge: String = if extension.is_empty() || extension.chars().count() > 4 {
        "FILE".into()
    } else {
        extension.to_uppercase()
    };
    let original = media.original();
    let online = store.connection() == ConnectionState::Online;
    let line = action_line(store.download_state(&original), online);
    div()
        .debug_selector(move || format!("msg-{ts}-file"))
        .flex()
        .items_center()
        .gap(px(10.))
        .w(px(300.))
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
                .child(
                    div()
                        .truncate()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(name.to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .child(human_size(size)),
                )
                .child(action_row(ctx, ts, original, name, line)),
        )
}

/// Status text and the button of a file card. The row has one height in every state, so the message
/// does not change size while a download runs.
fn action_row(ctx: &RowContext, ts: i64, original: MediaRef, name: &str, line: ActionLine) -> Div {
    let ActionLine {
        status,
        action,
        enabled,
    } = line;
    let suffix = match action {
        Action::Download | Action::DownloadAgain => "download",
        Action::Cancel => "cancel",
        Action::Retry => "retry",
    };
    let saved = action == Action::DownloadAgain;
    let button = div()
        .id(SharedString::from(format!("file-{suffix}-{ts}")))
        .debug_selector(move || format!("msg-{ts}-{suffix}"))
        .flex_none()
        .child(action.label());
    let button = if enabled {
        let (store, name) = (ctx.store.clone(), name.to_owned());
        button
            .text_color(rgb(ACCENT))
            .cursor_pointer()
            .on_click(move |_, _, cx| match action {
                Action::Cancel => store.read(cx).cancel_download(&original),
                Action::Download | Action::Retry | Action::DownloadAgain => {
                    store.update(cx, |store, cx| {
                        store.download(original.clone(), Some(name.clone()), cx)
                    });
                }
            })
    } else {
        button.text_color(rgb(MUTED)).cursor_default()
    };
    let kind = if saved { "saved" } else { "status" };
    div()
        .h(px(18.))
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.))
        .children(status.map(|text| {
            div()
                .debug_selector(move || format!("msg-{ts}-{kind}"))
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(rgb(MUTED))
                .child(text)
        }))
        .child(button)
}
