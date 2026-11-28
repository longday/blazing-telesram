//! One timeline row: day separator, grouped bubble, attachments, reaction chips.

use crate::desktop::{Desktop, PickState};
use crate::format::{
    Clock, avatar_color, clock_time, day_key, day_label, duration_label, duration_size_line,
    fit_thumbnail, human_size,
};
use crate::player::{PlayFailure, Player, VoiceKey, VoicePhase, VoiceView, failure_label};
use crate::selection::{Pos, eligible};
use crate::store::{ChatStore, DownloadState};
use crate::text_lines::{caret_for, glyph_for, highlight_rects, paragraphs_of};
use crate::theme::*;
use crate::viewer::{Action, ActionLine, REVEAL_LABEL, action_line};
use crate::widgets::avatar;
use crate::yandex_backend::is_video_name;
use bt_chat_core::model::{
    ChatId, ChatKind, ConnectionState, Delivery, DownloadFailure, GalleryItem, MediaRef, Message,
    MessageBody, MessageTs, ReplyQuote,
};
use bt_chat_core::reactions::{NEUTRAL_GLYPH, ReactionCount, emoji};
use gpui::{prelude::*, *};
use std::cell::RefCell;
use std::ffi::OsStr;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

const GROUP_GAP_US: i64 = 5 * 60 * 1_000_000;
const BUBBLE_MAX: f32 = 480.;
/// The gutter that holds the check of a row in message mode.
const CHECK_GUTTER: f32 = 28.;
const BUBBLE_PAD_X: f32 = 12.;
const THUMB_MAX: f32 = 320.;
const AVATAR: f32 = 36.;
/// Width available to the content of a bubble.
const CONTENT_W: f32 = BUBBLE_MAX - 2. * BUBBLE_PAD_X;
const GRID_GAP: f32 = 4.;
/// Bars of a voice bubble's waveform (design D7).
const VOICE_BARS: usize = 28;
const VOICE_CIRCLE: f32 = 40.;
const VOICE_GAP: f32 = 10.;
const VOICE_BAR_W: f32 = 3.;
const VOICE_BAR_PITCH: f32 = 6.;
const VOICE_BARS_W: f32 = VOICE_BARS as f32 * VOICE_BAR_PITCH;
/// Fixed width of a voice bubble's content (design D7), so it never changes size between states.
const VOICE_W: f32 = 300.;
/// The slot of the time label: what is left of the content after the circle, its gap and the bars. A
/// failure text takes its place and wraps inside it in two short lines.
const VOICE_TIME_W: f32 = VOICE_W - VOICE_CIRCLE - VOICE_GAP - VOICE_BARS_W;
/// Bar height range in px; a message without a waveform shows `VOICE_BAR_FLAT` bars.
const VOICE_BAR_MIN: f32 = 4.;
const VOICE_BAR_MAX: f32 = 26.;
const VOICE_BAR_FLAT: f32 = 8.;
/// The tallest level of a message's waveform (levels run 0 to 15, 0 silence).
const WAVEFORM_MAX_LEVEL: u8 = 15;
/// Name offered to the Download action and to the playback copy when the message has none.
const VOICE_NAME: &str = "Голосовое сообщение";
/// Width of a video card and the circle that holds its play control (design D8).
const VIDEO_W: f32 = 300.;
const VIDEO_CIRCLE: f32 = 44.;

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

/// The text layouts painted in the latest frame, by message, and the bounds of the message rows they
/// sit in. A canvas before the list clears it in its prepaint; every row reports its bounds in its
/// prepaint (`rows`) and every row with selectable text pushes its layout while painting (`texts`), so
/// `texts` holds only measured and prepainted layouts (gpui panics on a hit test of any other). `rows`
/// is the registry's own copy: `RowBounds` is drained by `check_read`.
#[derive(Default)]
pub struct TextRegistry {
    pub texts: Vec<(MessageTs, TextLayout)>,
    pub rows: Vec<(MessageTs, Bounds<Pixels>)>,
}

impl TextRegistry {
    pub fn clear(&mut self) {
        self.texts.clear();
        self.rows.clear();
    }
}

pub type TextLayouts = Rc<RefCell<TextRegistry>>;

/// What a row needs besides the store's data: the entities its clicks act on.
#[derive(Clone)]
pub struct RowContext {
    pub store: Entity<ChatStore>,
    pub player: Entity<Player>,
    pub desktop: WeakEntity<Desktop>,
    pub chat: ChatId,
    pub clock: Clock,
    pub rows: RowBounds,
    pub texts: TextLayouts,
    pub picking: Entity<PickState>,
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
    let mode = ctx.picking.read(cx).marked();
    let (in_mode, marked) = (mode.is_some(), mode.is_some_and(|set| set.contains(ts)));

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
            let (rows, texts) = (ctx.rows.clone(), ctx.texts.clone());
            move |children, _, _| {
                if let (Some(first), Some(last)) = (children.first(), children.last()) {
                    let bounds = Bounds::from_corners(first.origin, last.bottom_right());
                    rows.borrow_mut().push((ts, bounds));
                    texts.borrow_mut().rows.push((ts, bounds));
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
                .when(in_mode, |d| {
                    // The gutter of the check takes the room the left padding leaves; the row's height
                    // does not change.
                    d.relative().pl(px(16. + CHECK_GUTTER))
                })
                .when(marked, |d| {
                    d.debug_selector(move || format!("msg-{ts}-marked"))
                        .bg(rgba(MARKED_ROW))
                })
                .when(in_mode, |d| d.child(check_gutter(ts, marked)))
                .children(avatar_slot)
                .child(bubble(ctx, store, cx, m, in_group_chat && group_start))
                .when(in_mode, |d| d.child(mark_layer(ctx, ts))),
        )
        .into_any_element()
}

/// The ○ or ✓ at the left edge of a row in message mode (D11), `msg-{ts}-check`.
fn check_gutter(ts: MessageTs, marked: bool) -> Div {
    div()
        .debug_selector(move || format!("msg-{ts}-check"))
        .absolute()
        .left(px(12.))
        .top_0()
        .bottom_0()
        .w(px(CHECK_GUTTER))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(18.))
        .text_color(rgb(if marked { ACCENT } else { CHECK_OFF }))
        .child(if marked { "✓" } else { "○" })
}

/// The transparent layer over a whole row in message mode: it blocks everything beneath it (chips,
/// pictures, play buttons, links, the text) and turns a left click into a mark; Shift marks the range.
/// A click on a row that cannot be marked does nothing.
fn mark_layer(ctx: &RowContext, ts: MessageTs) -> Div {
    let (desktop, chat) = (ctx.desktop.clone(), ctx.chat.clone());
    div()
        .debug_selector(move || format!("msg-{ts}-layer"))
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .occlude()
        .on_mouse_down(MouseButton::Left, move |event, _, cx| {
            cx.stop_propagation();
            desktop
                .update(cx, |desktop, cx| {
                    desktop.toggle_message(chat.clone(), ts, event.modifiers.shift, cx)
                })
                .ok();
        })
}

fn bubble(ctx: &RowContext, store: &ChatStore, cx: &App, m: &Message, show_name: bool) -> Div {
    let ts = m.timestamp;
    let (desktop, chat) = (ctx.desktop.clone(), ctx.chat.clone());
    let markable = eligible(m);
    div()
        .on_mouse_down(MouseButton::Right, move |event, _, cx| {
            desktop
                .update(cx, |desktop, cx| {
                    desktop.open_menu(chat.clone(), ts, event.position, cx)
                })
                .ok();
        })
        // A primary-modifier click on a message that can be marked enters message mode (D11); inside
        // the mode the row's own layer takes every click first.
        .when(markable, |d| {
            let (desktop, chat) = (ctx.desktop.clone(), ctx.chat.clone());
            d.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                if event.modifiers.secondary() {
                    cx.stop_propagation();
                    desktop
                        .update(cx, |desktop, cx| {
                            desktop.enter_mode(chat.clone(), ts, window, cx)
                        })
                        .ok();
                }
            })
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
        .child(body(ctx, store, cx, m))
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
                .when(shows_important_marker(m), |d| {
                    d.child(
                        div()
                            .debug_selector(move || format!("msg-{ts}-important"))
                            .text_color(rgb(IMPORTANT_MARK))
                            .child("⚡"),
                    )
                })
                .child(clock_time(ts, ctx.clock))
                .when(m.outgoing, |d| d.child(delivery_mark(ts, &m.delivery))),
        )
}

/// The "⚡" marker is for a live message the server flags as important; a deleted one shows none.
fn shows_important_marker(m: &Message) -> bool {
    m.important && !matches!(m.body, MessageBody::Deleted)
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

fn body(ctx: &RowContext, store: &ChatStore, cx: &App, m: &Message) -> AnyElement {
    let ts = m.timestamp;
    match &m.body {
        MessageBody::Text(text) => {
            selectable_text(ctx, store, cx, ts, text, "text").into_any_element()
        }
        MessageBody::Image {
            media,
            width,
            height,
            caption,
            ..
        } => attachment(
            ctx,
            store,
            cx,
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
            attachment(ctx, store, cx, ts, pictures, caption.as_deref())
        }
        MessageBody::File { media, name, size } => {
            file_card(ctx, store, ts, media, name, *size).into_any_element()
        }
        MessageBody::Voice {
            media,
            duration_ms,
            name,
            waveform,
            ..
        } => voice_bubble(
            ctx,
            store,
            cx,
            m,
            (media, *duration_ms, name.as_deref(), waveform.as_deref()),
        )
        .into_any_element(),
        MessageBody::Video {
            media,
            name,
            size,
            duration_ms,
            ..
        } => video_card(ctx, store, cx, m, (media, name, *size, *duration_ms)).into_any_element(),
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
fn attachment(
    ctx: &RowContext,
    store: &ChatStore,
    cx: &App,
    ts: MessageTs,
    pictures: Option<AnyElement>,
    caption: Option<&str>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .children(pictures)
        .when_some(caption, |d, caption| {
            d.child(selectable_text(ctx, store, cx, ts, caption, "caption"))
        })
        .into_any_element()
}

/// The text of a message that can be selected (D3), `msg-{ts}-{suffix}`. The bubble is content-sized, so
/// gpui measures its text at max-content width and keeps that single-line height even after the bubble is
/// clamped; a max width on the text itself makes the measurement wrap.
///
/// The text itself is an unchanged `StyledText`; the selection is painted as rectangles behind it by a
/// zero-size canvas placed before it, which also registers the painted layout for the caret mapping of
/// drags (spike 0.1: highlighting through the text runs would reshape the text). Mouse down on the text
/// puts the caret from the registered layout and hands the gesture to `Desktop::text_down`.
fn selectable_text(
    ctx: &RowContext,
    store: &ChatStore,
    cx: &App,
    ts: MessageTs,
    text: &str,
    suffix: &'static str,
) -> Div {
    let range = store.timeline(&ctx.chat).and_then(|timeline| {
        ctx.picking
            .read(cx)
            .text()?
            .range_in(&timeline.messages, ts)
    });
    let text = SharedString::from(text.to_owned());
    let styled = StyledText::new(text.clone());
    let layout = styled.layout().clone();
    let (desktop, chat) = (ctx.desktop.clone(), ctx.chat.clone());
    let registry = ctx.texts.clone();
    div()
        .debug_selector(move || format!("msg-{ts}-{suffix}"))
        .relative()
        .max_w(px(CONTENT_W))
        .text_size(px(15.))
        .line_height(px(21.))
        .cursor(CursorStyle::IBeam)
        .on_mouse_down(MouseButton::Left, {
            let (registry, text) = (registry.clone(), text.clone());
            move |event, window, cx| {
                // A primary-modifier click is the bubble's: it enters message mode and starts no drag.
                if event.modifiers.secondary() {
                    return;
                }
                // The click is the text's: the timeline's own handler for a click elsewhere must not clear it.
                cx.stop_propagation();
                let painted = registry
                    .borrow()
                    .texts
                    .iter()
                    .find(|(t, _)| *t == ts)
                    .map(|(_, layout)| layout.clone());
                let Some(painted) = painted else { return };
                // A caret goes to the nearer side of a glyph; a word or a paragraph is the one the
                // glyph under the pointer belongs to, so the right half of a word's last letter is
                // still that word.
                let offset = if event.click_count >= 2 {
                    glyph_for(&painted, event.position, &text)
                } else {
                    caret_for(&painted, event.position, &text)
                };
                let at = Pos { ts, offset };
                desktop
                    .update(cx, |desktop, cx| {
                        desktop.text_down(
                            chat.clone(),
                            at,
                            event.click_count,
                            event.modifiers.shift,
                            window,
                            cx,
                        )
                    })
                    .ok();
            }
        })
        .child(
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    registry.borrow_mut().texts.push((ts, layout.clone()));
                    let Some(range) = range.filter(|r| !r.is_empty()) else {
                        return;
                    };
                    let (origin, line_height) = (layout.bounds().origin, layout.line_height());
                    for (line, left, right) in
                        highlight_rects(&paragraphs_of(&layout, &text), range)
                    {
                        let top = origin.y + line_height * line as f32;
                        window.paint_quad(fill(
                            Bounds::new(
                                point(origin.x + px(left), top),
                                size(px(right - left), line_height),
                            ),
                            rgba(SELECTION),
                        ));
                    }
                },
            )
            .absolute()
            .size_0(),
        )
        .child(styled)
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
/// button, `-reveal` for the «Показать в Finder» link (only while the file is saved), `-saved` for
/// the "saved as" text and `-status` for any other text.
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

/// What the circle and the size line of a video card show for one player state (design D8). Pure, so
/// every state's text is testable without a window.
#[derive(Debug, PartialEq, Eq)]
struct VideoFace {
    glyph: &'static str,
    line: String,
}

/// `progress` is what the store knows of the playback copy while it is fetched: the bytes received and
/// the total when it is known. Loading shows the progress and the cancel glyph, a failure its reason,
/// an idle card whose play is `unavailable` the offline text, any other idle card the duration and
/// size. The card has no playing or paused state: the system player takes over.
fn video_face(
    phase: VoicePhase,
    progress: Option<(u64, Option<u64>)>,
    duration_ms: Option<u64>,
    size: u64,
    unavailable: bool,
) -> VideoFace {
    let (glyph, line) = match phase {
        VoicePhase::Loading => {
            let line = match progress {
                Some((received, Some(total))) => format!(
                    "Загрузка для просмотра: {} из {}",
                    human_size(received),
                    human_size(total)
                ),
                Some((received, None)) => {
                    format!("Загрузка для просмотра: {}", human_size(received))
                }
                None => "Загрузка для просмотра…".to_owned(),
            };
            ("✕", line)
        }
        VoicePhase::Failed(failure) => ("▶", failure_label(failure).to_owned()),
        VoicePhase::Idle | VoicePhase::Playing | VoicePhase::Paused if unavailable => (
            "▶",
            failure_label(PlayFailure::Download(DownloadFailure::Offline)).to_owned(),
        ),
        VoicePhase::Idle | VoicePhase::Playing | VoicePhase::Paused => {
            ("▶", duration_size_line(duration_ms, Some(size)))
        }
    };
    VideoFace { glyph, line }
}

/// A video message the server has gets a player; one still in the outbox (or refused) has no file to
/// fetch, so its card is inert.
fn video_has_player(m: &Message) -> bool {
    matches!(m.body, MessageBody::Video { .. }) && m.delivery == Delivery::Sent
}

/// The last extension of the file name of `path` is a whitelisted video one: the check `Player` makes
/// on a reused copy before it hands it to the system.
fn is_video_path(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(is_video_name)
}

/// The video card of design D8: a 44 px circle that plays («▶», or «✕» that cancels while the copy is
/// fetched), the name, the line with the duration and size (progress or the failure in its place), and
/// below the Download action row of the original, as on a file card. Debug selectors: `msg-{ts}-video`
/// (the card), `-video-play` (the circle), `-video-status` (the size line), and the action row's own.
/// The circle's element id is `video-play-{ts}`. Without `video_has_player` the circle is dimmed, does
/// nothing, and the action row is left out.
fn video_card(
    ctx: &RowContext,
    store: &ChatStore,
    cx: &App,
    m: &Message,
    video: (&MediaRef, &str, u64, Option<u64>),
) -> Div {
    let (media, name, size, duration_ms) = video;
    let ts = m.timestamp;
    let playable = video_has_player(m);
    let original = media.original();
    let playback = original.playback();
    let online = store.connection() == ConnectionState::Online;
    let copy = store.download_state(&playback);
    let unavailable = playable && play_unavailable(online, copy, is_video_path);
    let phase = if playable {
        ctx.player.read(cx).video(&original)
    } else {
        VoicePhase::Idle
    };
    let progress = match copy {
        Some(DownloadState::Running { received, total }) => Some((*received, *total)),
        _ => None,
    };
    let face = video_face(phase, progress, duration_ms, size, unavailable);
    let enabled = playable && circle_enabled(phase, unavailable);
    probe::begin_video(ts);
    let line = action_line(store.download_state(&original), online);
    let toggle = {
        let (player, original, name) = (ctx.player.clone(), original.clone(), name.to_owned());
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            player.update(cx, |player, cx| player.open_video(&original, &name, cx));
        }
    };
    div()
        .debug_selector(move || format!("msg-{ts}-video"))
        .flex()
        .items_center()
        .gap(px(10.))
        .w(px(VIDEO_W))
        .child(
            div()
                .id(SharedString::from(format!("video-play-{ts}")))
                .debug_selector(move || format!("msg-{ts}-video-play"))
                .size(px(VIDEO_CIRCLE))
                .flex_none()
                .rounded_full()
                .bg(rgb(if enabled { BADGE } else { BADGE_MUTED }))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(18.))
                .when(enabled, |d| {
                    d.cursor_pointer().on_click(probe::video_click(ts, toggle))
                })
                .child(probe::video_glyph(ts, face.glyph)),
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
                        .child(probe::video_name(ts, name)),
                )
                .child(
                    div()
                        .debug_selector(move || format!("msg-{ts}-video-status"))
                        .h(px(16.))
                        .truncate()
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .child(probe::video_line(ts, face.line)),
                )
                .when(playable, |d| {
                    d.child(action_row(ctx, ts, original, name, line))
                }),
        )
}

/// What the circle, the time slot and the lit bars show for one player state (design D7). Pure, so
/// every state's text is testable without a window.
#[derive(Debug, PartialEq, Eq)]
struct VoiceFace {
    glyph: &'static str,
    /// The time, or the failure text that takes its place.
    label: String,
    failed: bool,
    /// Bars before the playhead.
    lit: usize,
}

fn voice_face(view: &VoiceView, wire_ms: Option<u64>) -> VoiceFace {
    let total = shown_total_ms(wire_ms, view.total).map(duration_label);
    let total = total.unwrap_or_default();
    let elapsed = duration_label(view.position.as_millis() as u64);
    let lit = view.total.map_or(0, |total| lit_bars(view.position, total));
    let (glyph, label, failed, lit) = match view.phase {
        VoicePhase::Idle => ("▶", total, false, 0),
        VoicePhase::Loading => ("…", total, false, 0),
        VoicePhase::Playing => ("⏸", elapsed, false, lit),
        VoicePhase::Paused if view.position.is_zero() => ("▶", total, false, lit),
        VoicePhase::Paused => ("▶", elapsed, false, lit),
        VoicePhase::Failed(failure) => ("▶", failure_label(failure).to_owned(), true, 0),
    };
    VoiceFace {
        glyph,
        label,
        failed,
        lit,
    }
}

/// Offline and with no saved playback copy, play cannot work: the bubble says so before the click
/// instead of after it. A copy that is already saved plays offline as it does online.
fn play_unavailable(
    online: bool,
    copy: Option<&DownloadState>,
    accepts: impl Fn(&Path) -> bool,
) -> bool {
    // Online the copy does not matter, so the disk is asked only offline.
    !online && !usable_copy(copy, accepts)
}

/// A saved playback copy the player will really use: `Saved` and still a file on disk (the criterion of
/// `Player::toggle_voice` and `open_video`, which fetch again when it vanished) that `accepts` the path.
fn usable_copy(copy: Option<&DownloadState>, accepts: impl Fn(&Path) -> bool) -> bool {
    matches!(copy, Some(DownloadState::Saved(path)) if path.is_file() && accepts(path))
}

/// Whether the circle reacts to a click. Offline without a copy only a start (from `Idle` or `Failed`)
/// is unavailable; the cancel of a load that began before the connection dropped, and pause and resume,
/// always work. Pure, so the mapping is testable without a window.
fn circle_enabled(phase: VoicePhase, unavailable: bool) -> bool {
    match phase {
        VoicePhase::Idle | VoicePhase::Failed(_) => !unavailable,
        VoicePhase::Loading | VoicePhase::Playing | VoicePhase::Paused => true,
    }
}

/// `voice_face`, except that an idle bubble whose play is unavailable shows the offline text in place
/// of the duration. Loading, playing, paused and failed bubbles show their own state.
fn voice_face_with(view: &VoiceView, wire_ms: Option<u64>, unavailable: bool) -> VoiceFace {
    if unavailable && view.phase == VoicePhase::Idle {
        let offline = PlayFailure::Download(DownloadFailure::Offline);
        return VoiceFace {
            glyph: "▶",
            label: failure_label(offline).to_owned(),
            failed: true,
            lit: 0,
        };
    }
    voice_face(view, wire_ms)
}

/// The total the bubble shows: the length the message carries until the file is open, then the file's
/// own length, except that a difference of up to a second keeps the first figure so the label does not
/// flicker when the file opens (design D6).
fn shown_total_ms(wire_ms: Option<u64>, file: Option<Duration>) -> Option<u64> {
    let file_ms = file.map(|f| f.as_millis() as u64);
    match (wire_ms, file_ms) {
        (Some(wire), Some(file)) if wire.abs_diff(file) <= 1000 => Some(wire),
        (_, Some(file)) => Some(file),
        (wire, None) => wire,
    }
}

/// How many of the bars lie before the playhead; none for an unknown or empty total, all at the end.
fn lit_bars(position: Duration, total: Duration) -> usize {
    let total = total.as_millis();
    if total == 0 {
        return 0;
    }
    let lit = position.as_millis() * VOICE_BARS as u128 / total;
    lit.min(VOICE_BARS as u128) as usize
}

/// The waveform samples reduced to one value per bar: each bar takes the maximum of the samples it
/// spans, and with fewer samples than bars each sample spans several bars. `None` for no samples.
fn resample_waveform(samples: &[u8]) -> Option<[u8; VOICE_BARS]> {
    let n = samples.len();
    if n == 0 {
        return None;
    }
    let mut bars = [0; VOICE_BARS];
    for (i, bar) in bars.iter_mut().enumerate() {
        let start = i * n / VOICE_BARS;
        let end = ((i + 1) * n / VOICE_BARS).clamp(start + 1, n);
        *bar = samples[start..end].iter().copied().max().unwrap_or(0);
    }
    Some(bars)
}

/// Bar heights in px: the levels (0 to `WAVEFORM_MAX_LEVEL`, 0 silence) scaled into the bar range, so a
/// silent bar is still visible; flat bars when there is no waveform.
fn bar_heights(waveform: Option<&[u8]>) -> [f32; VOICE_BARS] {
    let Some(bars) = waveform.and_then(resample_waveform) else {
        return [VOICE_BAR_FLAT; VOICE_BARS];
    };
    bars.map(|level| {
        let level = f32::from(level.min(WAVEFORM_MAX_LEVEL));
        VOICE_BAR_MIN + (VOICE_BAR_MAX - VOICE_BAR_MIN) * level / f32::from(WAVEFORM_MAX_LEVEL)
    })
}

/// Only a message the server has gets a player: a voice message still in the outbox (or refused) has
/// no file to fetch, so its bubble is inert (design D7).
fn voice_has_player(m: &Message) -> bool {
    matches!(m.body, MessageBody::Voice { .. }) && m.delivery == Delivery::Sent
}

/// What the voice bubble hands to its elements, recorded at the construction sites in test builds (as the
/// pinned bar's `BAR_LINE` does for its text): every value passes through one of these functions on its
/// way into the element tree, so a test reads what was actually put there, not what was computed.
#[cfg(test)]
pub(crate) mod probe {
    use crate::theme::ACCENT;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// The glyph and the time text of the circle and the time slot, the bars built and those of them in
    /// the accent colour (the lit part), and whether the circle got its click handler.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub(crate) struct VoiceShown {
        pub glyph: String,
        pub label: String,
        pub bars: usize,
        pub accent: usize,
        pub enabled: bool,
    }

    thread_local! {
        /// By message timestamp; a render starts the entry afresh, so a test that removes it before a
        /// state change finds it again only if the row really rendered.
        pub(crate) static VOICE_SHOWN: RefCell<HashMap<i64, VoiceShown>> = RefCell::new(HashMap::new());
    }

    fn record(ts: i64, record: impl FnOnce(&mut VoiceShown)) {
        VOICE_SHOWN.with(|shown| record(shown.borrow_mut().entry(ts).or_default()));
    }

    pub(super) fn begin(ts: i64) {
        VOICE_SHOWN.with(|shown| shown.borrow_mut().insert(ts, VoiceShown::default()));
    }

    pub(super) fn glyph(ts: i64, glyph: &'static str) -> &'static str {
        record(ts, |s| s.glyph = glyph.to_owned());
        glyph
    }

    pub(super) fn label(ts: i64, label: String) -> String {
        record(ts, |s| s.label = label.clone());
        label
    }

    pub(super) fn bar(ts: i64, color: u32) -> u32 {
        record(ts, |s| {
            s.bars += 1;
            s.accent += usize::from(color == ACCENT);
        });
        color
    }

    pub(super) fn click<F>(ts: i64, handler: F) -> F {
        record(ts, |s| s.enabled = true);
        handler
    }

    /// The glyph, the name and the size line of a video card, and whether the circle got its click
    /// handler.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub(crate) struct VideoShown {
        pub glyph: String,
        pub name: String,
        pub line: String,
        pub enabled: bool,
    }

    thread_local! {
        /// By message timestamp, started afresh by every render of the card, as `VOICE_SHOWN`.
        pub(crate) static VIDEO_SHOWN: RefCell<HashMap<i64, VideoShown>> = RefCell::new(HashMap::new());
    }

    fn record_video(ts: i64, record: impl FnOnce(&mut VideoShown)) {
        VIDEO_SHOWN.with(|shown| record(shown.borrow_mut().entry(ts).or_default()));
    }

    pub(super) fn begin_video(ts: i64) {
        VIDEO_SHOWN.with(|shown| shown.borrow_mut().insert(ts, VideoShown::default()));
    }

    pub(super) fn video_glyph(ts: i64, glyph: &'static str) -> &'static str {
        record_video(ts, |s| s.glyph = glyph.to_owned());
        glyph
    }

    pub(super) fn video_name(ts: i64, name: &str) -> String {
        record_video(ts, |s| s.name = name.to_owned());
        name.to_owned()
    }

    pub(super) fn video_line(ts: i64, line: String) -> String {
        record_video(ts, |s| s.line = line.clone());
        line
    }

    pub(super) fn video_click<F>(ts: i64, handler: F) -> F {
        record_video(ts, |s| s.enabled = true);
        handler
    }
}

/// The same hooks in a non-test build: they hand their value on.
#[cfg(not(test))]
mod probe {
    pub(super) fn begin(_: i64) {}

    pub(super) fn glyph(_: i64, glyph: &'static str) -> &'static str {
        glyph
    }

    pub(super) fn label(_: i64, label: String) -> String {
        label
    }

    pub(super) fn bar(_: i64, color: u32) -> u32 {
        color
    }

    pub(super) fn click<F>(_: i64, handler: F) -> F {
        handler
    }

    pub(super) fn begin_video(_: i64) {}

    pub(super) fn video_glyph(_: i64, glyph: &'static str) -> &'static str {
        glyph
    }

    pub(super) fn video_name(_: i64, name: &str) -> String {
        name.to_owned()
    }

    pub(super) fn video_line(_: i64, line: String) -> String {
        line
    }

    pub(super) fn video_click<F>(_: i64, handler: F) -> F {
        handler
    }
}

/// The voice bubble of design D7: a circle that plays and pauses, the waveform with the played part
/// lit, the time (or the failure text), and below it the Download action row of the original. Every
/// slot has a fixed size, so the bubble is the same size in every state. Debug selectors:
/// `msg-{ts}-voice` (the bubble), `-voice-toggle` (the circle), `-voice-bars`, `-voice-time`, and the
/// action row's own. The circle's element id is `voice-toggle-{ts}`. Without `voice_has_player` the
/// circle is dimmed, does nothing, and the action row is left out.
fn voice_bubble(
    ctx: &RowContext,
    store: &ChatStore,
    cx: &App,
    m: &Message,
    voice: (&MediaRef, Option<u64>, Option<&str>, Option<&[u8]>),
) -> Div {
    let (media, duration_ms, name, waveform) = voice;
    let ts = m.timestamp;
    let playable = voice_has_player(m);
    let key = VoiceKey {
        chat: ctx.chat.clone(),
        timestamp: ts,
    };
    let original = media.original();
    let online = store.connection() == ConnectionState::Online;
    let unavailable =
        playable && play_unavailable(online, store.download_state(&original.playback()), |_| true);
    let view = if playable {
        ctx.player.read(cx).voice(&key)
    } else {
        VoiceView::default()
    };
    let face = voice_face_with(&view, duration_ms, unavailable);
    let enabled = playable && circle_enabled(view.phase, unavailable);
    probe::begin(ts);
    let line = action_line(store.download_state(&original), online);
    let toggle = {
        let (player, original, name) = (
            ctx.player.clone(),
            original.clone(),
            name.map(str::to_owned),
        );
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            player.update(cx, |player, cx| {
                player.toggle_voice(key.clone(), &original, name.as_deref(), cx)
            });
        }
    };
    let lit = face.lit;
    let bars = bar_heights(waveform)
        .into_iter()
        .enumerate()
        .map(|(i, height)| {
            div()
                .w(px(VOICE_BAR_W))
                .h(px(height))
                .flex_none()
                .rounded(px(VOICE_BAR_W / 2.))
                .bg(rgb(probe::bar(ts, if i < lit { ACCENT } else { MUTED })))
        });
    div()
        .debug_selector(move || format!("msg-{ts}-voice"))
        .w(px(VOICE_W))
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(VOICE_CIRCLE))
                .flex()
                .items_center()
                .child(
                    div()
                        .id(SharedString::from(format!("voice-toggle-{ts}")))
                        .debug_selector(move || format!("msg-{ts}-voice-toggle"))
                        .size(px(VOICE_CIRCLE))
                        .mr(px(VOICE_GAP))
                        .flex_none()
                        .rounded_full()
                        .bg(rgb(if enabled { BADGE } else { BADGE_MUTED }))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(16.))
                        .when(enabled, |d| {
                            d.cursor_pointer().on_click(probe::click(ts, toggle))
                        })
                        .child(probe::glyph(ts, face.glyph)),
                )
                .child(
                    div()
                        .debug_selector(move || format!("msg-{ts}-voice-bars"))
                        .w(px(VOICE_BARS_W))
                        .h(px(VOICE_CIRCLE))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(VOICE_BAR_PITCH - VOICE_BAR_W))
                        .children(bars),
                )
                .child(
                    div()
                        .debug_selector(move || format!("msg-{ts}-voice-time"))
                        .w(px(VOICE_TIME_W))
                        .h(px(VOICE_CIRCLE))
                        .flex_none()
                        .overflow_hidden()
                        .flex()
                        .items_center()
                        .text_size(px(if face.failed { 10. } else { 12. }))
                        .text_color(rgb(MUTED))
                        .child(
                            div()
                                .max_w(px(VOICE_TIME_W))
                                .child(probe::label(ts, face.label)),
                        ),
                ),
        )
        .when(playable, |d| {
            d.child(action_row(
                ctx,
                ts,
                original,
                name.unwrap_or(VOICE_NAME),
                line,
            ))
        })
}

/// Status text and the button of a file card. The row has one height in every state, so the message
/// does not change size while a download runs.
fn action_row(ctx: &RowContext, ts: i64, original: MediaRef, name: &str, line: ActionLine) -> Div {
    let ActionLine {
        status,
        action,
        enabled,
        reveal,
    } = line;
    let reveal_original = original.clone();
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
    let reveal_store = ctx.store.clone();
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
        .when(reveal, |row| {
            row.child(
                div()
                    .id(SharedString::from(format!("file-reveal-{ts}")))
                    .debug_selector(move || format!("msg-{ts}-reveal"))
                    .flex_none()
                    .text_color(rgb(ACCENT))
                    .cursor_pointer()
                    .child(REVEAL_LABEL)
                    .on_click(move |_, _, cx| {
                        reveal_store
                            .update(cx, |store, cx| store.reveal_download(&reveal_original, cx));
                    }),
            )
        })
        .child(button)
}

#[cfg(test)]
mod tests {
    use super::{
        CONTENT_W, VOICE_BAR_FLAT, VOICE_BAR_MAX, VOICE_BAR_MIN, VOICE_BARS, VOICE_BARS_W,
        VOICE_CIRCLE, VOICE_GAP, VOICE_TIME_W, VOICE_W, VideoFace, VoiceFace, WAVEFORM_MAX_LEVEL,
        bar_heights, circle_enabled, is_video_path, lit_bars, play_unavailable, resample_waveform,
        shown_total_ms, shows_important_marker, video_face, video_has_player, voice_face,
        voice_face_with, voice_has_player,
    };
    use crate::player::{PlayFailure, VoicePhase, VoiceView};
    use crate::store::DownloadState;
    use bt_chat_core::model::{
        ChatId, Delivery, DownloadFailure, MediaKind, MediaRef, Message, MessageBody, PayloadId,
        UserId,
    };
    use std::path::Path;
    use std::time::Duration;

    fn message(body: MessageBody, important: bool) -> Message {
        Message {
            chat: ChatId("0/0/c".into()),
            timestamp: 1,
            author: UserId("alice".into()),
            author_name: None,
            author_avatar: None,
            outgoing: false,
            body,
            edited: false,
            reply_to: None,
            forwarded_from: None,
            delivery: Delivery::Sent,
            reactions: Default::default(),
            important,
        }
    }

    #[test]
    fn the_important_marker_needs_the_flag_and_a_live_body() {
        let text = || MessageBody::Text("hi".into());
        assert!(!shows_important_marker(&message(text(), false)));
        assert!(shows_important_marker(&message(text(), true)));
        assert!(!shows_important_marker(&message(
            MessageBody::Deleted,
            true
        )));
    }

    fn view(phase: VoicePhase, position_ms: u64, total_ms: Option<u64>) -> VoiceView {
        VoiceView {
            phase,
            position: Duration::from_millis(position_ms),
            total: total_ms.map(Duration::from_millis),
        }
    }

    #[test]
    fn fewer_samples_than_bars_repeat_each_sample_in_order() {
        let bars = resample_waveform(&[10, 20, 30]).unwrap();
        assert_eq!((bars[0], bars[9], bars[10]), (10, 10, 20));
        assert_eq!((bars[18], bars[27]), (20, 30));
        assert!(bars.windows(2).all(|pair| pair[0] <= pair[1]));
        // One sample fills every bar.
        assert_eq!(resample_waveform(&[7]).unwrap(), [7; VOICE_BARS]);
    }

    #[test]
    fn as_many_samples_as_bars_map_one_to_one() {
        let samples: Vec<u8> = (0..VOICE_BARS as u8).map(|i| i % 16).collect();
        assert_eq!(resample_waveform(&samples).unwrap().as_slice(), samples);
    }

    #[test]
    fn more_samples_than_bars_take_the_maximum_of_each_span() {
        // 56 samples: two per bar; the spike sits in the second sample of bar 3.
        let mut samples = vec![1u8; 56];
        samples[7] = 15;
        let bars = resample_waveform(&samples).unwrap();
        assert_eq!(bars[3], 15);
        assert_eq!(bars.iter().filter(|&&bar| bar == 15).count(), 1);
        assert_eq!(bars[2], 1);
        // About 200 levels (and 100, 101): the last sample is not lost.
        for n in [100, 101, 200, 201] {
            let mut samples = vec![0u8; n];
            samples[n - 1] = 15;
            assert_eq!(resample_waveform(&samples).unwrap()[VOICE_BARS - 1], 15);
        }
    }

    #[test]
    fn an_empty_waveform_gives_flat_bars_like_none() {
        assert_eq!(resample_waveform(&[]), None);
        assert_eq!(bar_heights(Some(&[])), [VOICE_BAR_FLAT; VOICE_BARS]);
        assert_eq!(bar_heights(None), [VOICE_BAR_FLAT; VOICE_BARS]);
    }

    #[test]
    fn bar_heights_scale_the_levels_into_the_bar_range() {
        let level = |l: u8| bar_heights(Some(&[l; 40]))[0];
        // Silence is the shortest bar, the top level the tallest, never zero or NaN.
        assert_eq!(level(0), VOICE_BAR_MIN);
        assert_eq!(level(WAVEFORM_MAX_LEVEL), VOICE_BAR_MAX);
        assert!(level(1) > level(0) && level(8) > level(1) && level(15) > level(8));
        // A value above the documented range does not grow the bar past the maximum.
        assert_eq!(level(200), VOICE_BAR_MAX);
        let ramp: Vec<u8> = (0..=15).collect();
        assert!(
            bar_heights(Some(&ramp))
                .windows(2)
                .all(|pair| pair[0] <= pair[1])
        );
    }

    #[test]
    fn lit_bars_follow_the_position_and_clamp_at_the_ends() {
        let total = Duration::from_secs(14);
        let lit = |ms| lit_bars(Duration::from_millis(ms), total);
        assert_eq!(lit(0), 0);
        assert_eq!(lit(499), 0);
        assert_eq!(lit(500), 1);
        assert_eq!(lit(7000), 14);
        assert_eq!(lit(13_999), 27);
        assert_eq!(lit(14_000), VOICE_BARS);
        assert_eq!(lit(99_000), VOICE_BARS);
        assert_eq!(lit_bars(Duration::from_secs(3), Duration::ZERO), 0);
    }

    #[test]
    fn the_bubble_geometry_is_the_300_px_of_the_design() {
        assert_eq!(VOICE_W, 300.);
        assert_eq!(VOICE_BARS_W, 168.);
        // The circle, its 10 px gap, the 28 bars and the time slot fill the content exactly.
        assert_eq!(
            VOICE_CIRCLE + VOICE_GAP + VOICE_BARS_W + VOICE_TIME_W,
            VOICE_W
        );
        assert_eq!(VOICE_GAP, 10.);
        const { assert!(VOICE_TIME_W > 0.) };
        const { assert!(VOICE_W <= CONTENT_W) };
        const { assert!(VOICE_BAR_MAX <= VOICE_CIRCLE) };
    }

    #[test]
    fn only_a_sent_voice_message_gets_a_player() {
        let voice = || MessageBody::Voice {
            media: MediaRef {
                id: "b/1".into(),
                kind: MediaKind::File,
            },
            duration_ms: Some(5_000),
            size: 9,
            name: None,
            waveform: None,
        };
        let mut m = message(voice(), false);
        assert!(voice_has_player(&m));
        m.delivery = Delivery::Pending(PayloadId("p".into()));
        assert!(!voice_has_player(&m));
        m.delivery = Delivery::Failed {
            payload: PayloadId("p".into()),
            reason: "no".into(),
        };
        assert!(!voice_has_player(&m));
        // A deleted body is not a voice body, so it never has one either.
        assert!(!voice_has_player(&message(MessageBody::Deleted, false)));
        assert!(!voice_has_player(&message(
            MessageBody::Text("hi".into()),
            false
        )));
    }

    #[test]
    fn before_the_file_opens_the_time_is_the_message_duration() {
        let idle = voice_face(&view(VoicePhase::Idle, 0, None), Some(14_900));
        assert_eq!(
            idle,
            VoiceFace {
                glyph: "▶",
                label: "0:14".into(),
                failed: false,
                lit: 0
            }
        );
        // No duration on the wire and no file: nothing until it is known.
        assert_eq!(voice_face(&view(VoicePhase::Idle, 0, None), None).label, "");
        let loading = voice_face(&view(VoicePhase::Loading, 0, None), Some(67_000));
        assert_eq!(
            (loading.glyph, loading.label.as_str(), loading.lit),
            ("…", "1:07", 0)
        );
    }

    #[test]
    fn playing_and_paused_show_the_elapsed_time_and_lit_bars() {
        let playing = voice_face(
            &view(VoicePhase::Playing, 3_400, Some(14_000)),
            Some(14_000),
        );
        assert_eq!(
            playing,
            VoiceFace {
                glyph: "⏸",
                label: "0:03".into(),
                failed: false,
                lit: 6
            }
        );
        let paused = voice_face(&view(VoicePhase::Paused, 7_000, Some(14_000)), Some(14_000));
        assert_eq!(
            (paused.glyph, paused.label.as_str(), paused.lit),
            ("▶", "0:07", 14)
        );
        // Paused at the very start shows the total again.
        let at_zero = voice_face(&view(VoicePhase::Paused, 0, Some(14_000)), Some(14_000));
        assert_eq!((at_zero.label.as_str(), at_zero.lit), ("0:14", 0));
        // Playing at the very start shows the elapsed time.
        let started = voice_face(&view(VoicePhase::Playing, 0, Some(14_000)), Some(14_000));
        assert_eq!(started.label, "0:00");
    }

    #[test]
    fn the_total_keeps_the_wire_figure_unless_the_file_differs_by_over_a_second() {
        let file = Some(Duration::from_millis(14_800));
        assert_eq!(shown_total_ms(Some(14_000), file), Some(14_000));
        assert_eq!(shown_total_ms(Some(13_800), file), Some(13_800));
        assert_eq!(shown_total_ms(Some(13_799), file), Some(14_800));
        assert_eq!(shown_total_ms(None, file), Some(14_800));
        assert_eq!(shown_total_ms(Some(5_000), None), Some(5_000));
        assert_eq!(shown_total_ms(None, None), None);
    }

    #[test]
    fn a_failure_text_takes_the_place_of_the_time() {
        let failed = |failure| view(VoicePhase::Failed(failure), 0, None);
        let cases = [
            (PlayFailure::Download(DownloadFailure::Offline), "Нет сети"),
            (
                PlayFailure::Download(DownloadFailure::NotFound),
                "Файл недоступен",
            ),
            (PlayFailure::Unsupported, "Формат не поддерживается"),
            (PlayFailure::NoOutput, "Нет вывода звука"),
        ];
        for (failure, text) in cases {
            let face = voice_face(&failed(failure), Some(14_000));
            assert_eq!(
                (face.glyph, face.label.as_str(), face.failed, face.lit),
                ("▶", text, true, 0)
            );
        }
    }

    #[test]
    fn play_is_unavailable_only_offline_without_a_usable_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("copy.ogg");
        std::fs::write(&path, b"x").unwrap();
        let saved = DownloadState::Saved(path.clone());
        let running = DownloadState::Running {
            received: 1,
            total: None,
        };
        let any = |_: &Path| true;
        assert!(play_unavailable(false, None, any));
        assert!(play_unavailable(false, Some(&running), any));
        assert!(!play_unavailable(false, Some(&saved), any));
        assert!(!play_unavailable(true, None, any));
        assert!(!play_unavailable(true, Some(&saved), any));
        // Offline with a copy the caller refuses is as good as none.
        assert!(play_unavailable(false, Some(&saved), |_| false));
        // Online the disk is not asked at all, neither for `is_file` nor for the caller's check.
        assert!(!play_unavailable(true, Some(&saved), |_| unreachable!()));
    }

    #[test]
    fn a_saved_copy_deleted_from_the_disk_is_unavailable_offline_like_the_player_says() {
        // The player's criterion (`Saved` and `is_file()`): the store still says `Saved` after the file
        // was removed, and offline the bubble must show «Нет сети» with a disabled circle, because a
        // click would find no copy and no connection.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("copy.ogg");
        std::fs::write(&path, b"x").unwrap();
        let saved = DownloadState::Saved(path.clone());
        let idle = view(VoicePhase::Idle, 0, None);

        let unavailable = play_unavailable(false, Some(&saved), |_| true);
        assert!(!unavailable);
        assert_eq!(
            voice_face_with(&idle, Some(14_000), unavailable).label,
            "0:14"
        );
        assert!(circle_enabled(VoicePhase::Idle, unavailable));

        std::fs::remove_file(&path).unwrap();
        let unavailable = play_unavailable(false, Some(&saved), |_| true);
        assert!(unavailable);
        let face = voice_face_with(&idle, Some(14_000), unavailable);
        assert_eq!((face.glyph, face.label.as_str()), ("▶", "Нет сети"));
        assert!(!circle_enabled(VoicePhase::Idle, unavailable));
        // Online the same state is startable: the click fetches the copy again.
        assert!(!play_unavailable(true, Some(&saved), |_| true));
    }

    #[test]
    fn offline_without_a_copy_blocks_only_starting_not_cancel_pause_or_resume() {
        let failed = VoicePhase::Failed(PlayFailure::Unsupported);
        for phase in [VoicePhase::Idle, failed] {
            assert!(!circle_enabled(phase, true));
            assert!(circle_enabled(phase, false));
        }
        for phase in [VoicePhase::Loading, VoicePhase::Playing, VoicePhase::Paused] {
            assert!(circle_enabled(phase, true), "{phase:?} offline");
            assert!(circle_enabled(phase, false), "{phase:?} online");
        }
    }

    #[test]
    fn an_idle_bubble_says_offline_before_the_click_when_play_is_unavailable() {
        let idle = view(VoicePhase::Idle, 0, None);
        let face = voice_face_with(&idle, Some(14_000), true);
        assert_eq!(
            (face.glyph, face.label.as_str(), face.failed, face.lit),
            ("▶", "Нет сети", true, 0)
        );
        // Available: the duration, exactly as `voice_face`.
        assert_eq!(
            voice_face_with(&idle, Some(14_000), false),
            voice_face(&idle, Some(14_000))
        );
        // A bubble that is playing or paused (it has its copy) keeps its own state.
        for phase in [VoicePhase::Playing, VoicePhase::Paused, VoicePhase::Loading] {
            let state = view(phase, 3_000, Some(14_000));
            assert_eq!(
                voice_face_with(&state, Some(14_000), true),
                voice_face(&state, Some(14_000))
            );
        }
    }

    fn video_msg(delivery: Delivery) -> Message {
        let mut m = message(
            MessageBody::Video {
                media: MediaRef {
                    id: "b/2".into(),
                    kind: MediaKind::File,
                },
                name: "a.mp4".into(),
                size: 12 << 20,
                width: 0,
                height: 0,
                duration_ms: Some(42_000),
            },
            false,
        );
        m.delivery = delivery;
        m
    }

    #[test]
    fn an_idle_video_card_shows_the_duration_and_size_or_the_size_alone() {
        let face = |duration, size| video_face(VoicePhase::Idle, None, duration, size, false);
        assert_eq!(
            face(Some(42_000), 12 << 20),
            VideoFace {
                glyph: "▶",
                line: "0:42 · 12 МБ".into()
            }
        );
        assert_eq!(face(None, 12 << 20).line, "12 МБ");
        assert_eq!(face(None, 0).line, "0 Б");
    }

    #[test]
    fn a_loading_video_card_shows_the_progress_and_the_cancel_glyph() {
        let loading =
            |progress| video_face(VoicePhase::Loading, progress, Some(42_000), 80 << 20, false);
        let (some, unknown, none) = (
            loading(Some((3 << 20, Some(80 << 20)))),
            loading(Some((3 << 20, None))),
            loading(None),
        );
        assert_eq!(
            (some.glyph, some.line.as_str()),
            ("✕", "Загрузка для просмотра: 3,0 МБ из 80 МБ")
        );
        assert_eq!(unknown.line, "Загрузка для просмотра: 3,0 МБ");
        assert_eq!(none.line, "Загрузка для просмотра…");
        // Offline does not hide a load that is running: it can still be cancelled.
        assert_eq!(
            video_face(VoicePhase::Loading, None, None, 1, true).glyph,
            "✕"
        );
    }

    #[test]
    fn a_failed_video_card_shows_the_reason_in_the_size_line_and_retries_with_the_circle() {
        let failed =
            |failure| video_face(VoicePhase::Failed(failure), None, Some(42_000), 1, false);
        let cases = [
            (PlayFailure::Download(DownloadFailure::Network), "Нет сети"),
            (
                PlayFailure::Download(DownloadFailure::NotFound),
                "Файл недоступен",
            ),
            (PlayFailure::Unsupported, "Формат не поддерживается"),
        ];
        for (failure, text) in cases {
            let face = failed(failure);
            assert_eq!((face.glyph, face.line.as_str()), ("▶", text));
            assert!(circle_enabled(VoicePhase::Failed(failure), false));
        }
    }

    #[test]
    fn an_idle_video_card_says_offline_when_play_is_unavailable() {
        let face = video_face(VoicePhase::Idle, None, Some(42_000), 12 << 20, true);
        assert_eq!((face.glyph, face.line.as_str()), ("▶", "Нет сети"));
        assert!(!circle_enabled(VoicePhase::Idle, true));
        assert!(circle_enabled(VoicePhase::Loading, true));
    }

    #[test]
    fn offline_a_video_copy_is_usable_only_if_it_exists_and_passes_the_whitelist() {
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("copy.MP4");
        let wrong = dir.path().join("copy.command");
        std::fs::write(&ok, b"x").unwrap();
        std::fs::write(&wrong, b"x").unwrap();
        let unavailable = |path: &Path| {
            play_unavailable(
                false,
                Some(&DownloadState::Saved(path.to_path_buf())),
                is_video_path,
            )
        };
        assert!(!unavailable(&ok));
        assert!(unavailable(&wrong));
        assert!(unavailable(&dir.path().join("gone.mp4")));
        std::fs::remove_file(&ok).unwrap();
        assert!(unavailable(&ok));
        assert!(is_video_path(Path::new("/x/a.mov")));
        assert!(!is_video_path(Path::new("/x/mp4")));
    }

    #[test]
    fn only_a_sent_video_message_gets_a_player() {
        assert!(video_has_player(&video_msg(Delivery::Sent)));
        assert!(!video_has_player(&video_msg(Delivery::Pending(PayloadId(
            "p".into()
        )))));
        assert!(!video_has_player(&message(MessageBody::Deleted, false)));
    }
}
