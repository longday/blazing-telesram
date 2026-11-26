//! The list of the people who reacted to a message: one flat list in server order, read-only.
//!
//! What it shows is a pure function of the store's `ReactorsState`, the connection and the message's
//! known reaction total (`body`); `render` turns that into the popover surface. Where the surface is
//! placed, how it is closed and what its two buttons do belong to the host, which passes the button
//! actions in. Nothing here decides whether another page exists: `More` follows the store's
//! `exhausted`, `loading_more` and `more_failed`, never the total (design D5, D6).

use crate::format::{Clock, clock_time, day_label};
use crate::store::{ChatStore, MESSAGE_GONE, ReactorsState, ReactorsView};
use crate::theme::*;
use crate::widgets::{avatar, button};
use bt_chat_core::model::{ConnectionState, Reactor};
use bt_chat_core::reactions::{NEUTRAL_GLYPH, emoji};
use gpui::{prelude::*, *};

pub const WIDTH: f32 = 320.;
const MAX_HEIGHT: f32 = 420.;
const ROW_HEIGHT: f32 = 48.;

/// Shown for an entry without a name; never a blank and never a raw user id.
pub const UNKNOWN_NAME: &str = "Неизвестный";

/// What the list shows below its title.
#[derive(Clone, Debug, PartialEq)]
pub enum Body<'a> {
    /// The first page is on its way: never an empty list.
    Loading,
    /// The client has no connection, whether the list failed for that or is merely on screen from
    /// before: entries of an earlier time are not presented as current.
    Offline,
    /// The server refused or the request failed; the text is the reason.
    Failed(&'a str),
    /// The message was deleted while the list was open: no entries, and nothing to retry.
    Gone,
    /// `shown` entries (`rows`) are listed in server order. `skipped` entries had no identity;
    /// `total` is the message's known reaction count, when the message is known.
    Ready {
        rows: &'a [Reactor],
        shown: usize,
        total: Option<usize>,
        skipped: usize,
        more: More<'a>,
    },
}

/// The row at the end of a shown list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum More<'a> {
    /// The server has no more entries.
    Hidden,
    /// «Показать ещё».
    Offer,
    /// The next page is out.
    Loading,
    /// The next page failed (the reason); the shown entries stay.
    Retry(&'a str),
}

/// What the list shows for `state` while the client is in `connection`; `total` is the message's
/// known reaction count. A loaded list is shown only while the client is online. `More` follows the
/// state's `exhausted`, `loading_more` and `more_failed`, and never `total`.
pub fn body(state: &ReactorsState, connection: ConnectionState, total: Option<usize>) -> Body<'_> {
    match state {
        ReactorsState::Loading => Body::Loading,
        ReactorsState::Failed { offline: true, .. } => Body::Offline,
        ReactorsState::Failed { reason, .. } if reason == MESSAGE_GONE => Body::Gone,
        ReactorsState::Failed { reason, .. } => Body::Failed(reason),
        ReactorsState::Ready { .. } if connection != ConnectionState::Online => Body::Offline,
        ReactorsState::Ready {
            rows,
            skipped,
            exhausted,
            loading_more,
            more_failed,
            ..
        } => Body::Ready {
            rows,
            shown: rows.len(),
            total,
            skipped: *skipped,
            more: if *exhausted {
                More::Hidden
            } else if *loading_more {
                More::Loading
            } else if let Some(reason) = more_failed {
                More::Retry(reason)
            } else {
                More::Offer
            },
        },
    }
}

/// The line over the entries. "N из M" only when entries are missing, that is when what is shown
/// and what could not be shown together fall short of the known total; "K без данных" whenever the
/// server reported entries without an identity. `None` for a list with nothing to declare.
pub fn header_text(shown: usize, skipped: usize, total: Option<usize>) -> Option<String> {
    let short = total.filter(|total| shown.saturating_add(skipped) < *total);
    match (short, skipped) {
        (None, 0) => None,
        (Some(total), 0) => Some(format!("{shown} из {total}")),
        (None, skipped) => Some(format!("{skipped} без данных")),
        (Some(total), skipped) => Some(format!("{shown} из {total} · {skipped} без данных")),
    }
}

/// When the person reacted: `HH:MM` for today, otherwise the day and `HH:MM`.
pub fn reacted_at(at: i64, clock: Clock) -> String {
    let time = clock_time(at, clock);
    match day_label(at, clock).as_str() {
        "Сегодня" => time,
        day => format!("{day}, {time}"),
    }
}

/// The text of one entry.
#[derive(Debug, PartialEq, Eq)]
struct RowText {
    name: String,
    own: bool,
    glyph: &'static str,
    when: Option<String>,
}

fn row_text(reactor: &Reactor, clock: Clock) -> RowText {
    RowText {
        name: reactor
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(UNKNOWN_NAME)
            .to_string(),
        own: reactor.me,
        glyph: emoji(reactor.code).unwrap_or(NEUTRAL_GLYPH),
        when: reactor.at.map(|at| reacted_at(at, clock)),
    }
}

/// The reactions the message is known to have, summed over the types; `None` when the message is
/// not in the loaded timeline.
fn known_total(store: &ChatStore, view: &ReactorsView) -> Option<usize> {
    let timeline = store.timeline(&view.chat)?;
    let at = timeline
        .messages
        .binary_search_by_key(&view.timestamp, |m| m.timestamp)
        .ok()?;
    Some(
        timeline.messages[at]
            .reactions
            .counts
            .iter()
            .map(|c| c.count as usize)
            .sum(),
    )
}

fn notice(selector: &'static str, text: impl Into<SharedString>) -> Div {
    div()
        .debug_selector(move || selector.into())
        .px(px(16.))
        .py(px(10.))
        .text_color(rgb(MUTED))
        .child(text.into())
}

fn retry_button(on_retry: impl Fn(&mut App) + 'static) -> Stateful<Div> {
    button("reactors-retry", "Повторить")
        .debug_selector(|| "reactors-retry".into())
        .on_click(move |_, _, cx| on_retry(cx))
}

/// One person: avatar (or initials), name (the own entry carries a marker), the emoji of the chosen
/// reaction and, when the server told it, the time.
/// Debug selectors: `reactor-<guid>`, `reactor-<guid>-avatar`, `-name`, `-own`, `-emoji`, `-time`.
fn row(store: &ChatStore, reactor: &Reactor, clock: Clock) -> Div {
    let text = row_text(reactor, clock);
    let tag = format!("reactor-{}", reactor.user.0);
    let (name_tag, own_tag, emoji_tag, time_tag, row_tag) = (
        format!("{tag}-name"),
        format!("{tag}-own"),
        format!("{tag}-emoji"),
        format!("{tag}-time"),
        tag.clone(),
    );
    div()
        .debug_selector(move || row_tag)
        .h(px(ROW_HEIGHT))
        .w_full()
        .flex_none()
        .px(px(16.))
        .flex()
        .items_center()
        .gap(px(10.))
        .child(avatar(
            store,
            reactor.avatar.as_ref(),
            &reactor.user.0,
            &text.name,
            32.,
            format!("{tag}-avatar"),
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .debug_selector(move || name_tag)
                                .min_w_0()
                                .truncate()
                                .child(text.name),
                        )
                        .when(text.own, |d| {
                            d.child(
                                div()
                                    .debug_selector(move || own_tag)
                                    .flex_none()
                                    .px(px(6.))
                                    .rounded_full()
                                    .bg(rgb(FIELD))
                                    .text_size(px(12.))
                                    .text_color(rgb(ACCENT))
                                    .child("Вы"),
                            )
                        }),
                )
                .when_some(text.when, |d, when| {
                    d.child(
                        div()
                            .debug_selector(move || time_tag)
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child(when),
                    )
                }),
        )
        .child(
            div()
                .debug_selector(move || emoji_tag)
                .flex_none()
                .text_size(px(20.))
                .child(text.glyph),
        )
}

/// The row at the end of the entries.
fn more_row(
    more: More<'_>,
    on_more: impl Fn(&mut App) + 'static,
    on_retry: impl Fn(&mut App) + 'static,
) -> Option<Div> {
    match more {
        More::Hidden => None,
        More::Offer => Some(
            div()
                .debug_selector(|| "reactors-more".into())
                .p(px(8.))
                .child(
                    button("reactors-more", "Показать ещё")
                        .text_color(rgb(ACCENT))
                        .on_click(move |_, _, cx| on_more(cx)),
                ),
        ),
        More::Loading => Some(notice("reactors-more", "Загрузка…")),
        More::Retry(reason) => Some(
            div()
                .debug_selector(|| "reactors-more".into())
                .p(px(8.))
                .flex()
                .flex_col()
                .items_start()
                .gap(px(6.))
                .child(
                    div()
                        .debug_selector(|| "reactors-more-error".into())
                        .px(px(8.))
                        .text_size(px(13.))
                        .text_color(rgb(MUTED))
                        .child(format!("Не удалось загрузить: {reason}")),
                )
                .child(retry_button(on_retry)),
        ),
    }
}

/// The list surface for the open list `view`; the host places it (a popover anchored where the menu
/// was) and closes it. `on_retry` runs for every «Повторить» (a failed or offline list, a failed next
/// page), `on_more` for «Показать ещё».
/// Debug selectors: `reactors-list`, `reactors-loading`, `reactors-offline`, `reactors-gone`, `reactors-error`,
/// `reactors-error-reason`, `reactors-retry`, `reactors-header`, `reactors-empty`, `reactors-more`,
/// `reactors-more-error` and the row selectors of `row`.
pub fn render(
    view: &ReactorsView,
    store: &ChatStore,
    clock: Clock,
    on_more: impl Fn(&mut App) + 'static,
    on_retry: impl Fn(&mut App) + 'static,
) -> Div {
    let content = match body(&view.state, store.connection(), known_total(store, view)) {
        Body::Loading => notice("reactors-loading", "Загрузка…").into_any_element(),
        Body::Offline => div()
            .flex()
            .flex_col()
            .items_start()
            .child(notice("reactors-offline", "Нет соединения"))
            .child(div().px(px(16.)).pb(px(10.)).child(retry_button(on_retry)))
            .into_any_element(),
        Body::Gone => notice("reactors-gone", "Сообщение удалено").into_any_element(),
        Body::Failed(reason) => div()
            .flex()
            .flex_col()
            .items_start()
            .child(notice("reactors-error", "Не удалось загрузить список"))
            .child(
                div()
                    .debug_selector(|| "reactors-error-reason".into())
                    .px(px(16.))
                    .pb(px(10.))
                    .text_size(px(13.))
                    .text_color(rgb(MUTED))
                    .child(reason.to_string()),
            )
            .child(div().px(px(16.)).pb(px(10.)).child(retry_button(on_retry)))
            .into_any_element(),
        Body::Ready {
            rows,
            shown,
            total,
            skipped,
            more,
        } => div()
            .min_h_0()
            .flex()
            .flex_col()
            .when_some(header_text(shown, skipped, total), |d, text| {
                d.child(
                    notice("reactors-header", text)
                        .flex_none()
                        .text_size(px(13.)),
                )
            })
            .when(rows.is_empty(), |d| {
                d.child(notice("reactors-empty", "Нет данных о реакциях"))
            })
            .child(
                div()
                    .id("reactors-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(rows.iter().map(|reactor| row(store, reactor, clock)))
                    .children(more_row(more, on_more, on_retry)),
            )
            .into_any_element(),
    };
    div()
        .debug_selector(|| "reactors-list".into())
        .w(px(WIDTH))
        .max_h(px(MAX_HEIGHT))
        .flex()
        .flex_col()
        .rounded(px(8.))
        .bg(rgb(PANEL))
        .border_1()
        .border_color(rgb(MENU_SEPARATOR))
        .shadow(vec![BoxShadow {
            color: hsla(0., 0., 0., 0.35),
            offset: point(px(2.), px(4.)),
            blur_radius: px(10.),
            spread_radius: px(0.),
        }])
        .child(
            div()
                .h(px(44.))
                .flex_none()
                .px(px(16.))
                .flex()
                .items_center()
                .border_b_1()
                .border_color(rgb(DIVIDER))
                .font_weight(FontWeight::MEDIUM)
                .child("Кто отреагировал"),
        )
        .child(content)
}

#[cfg(test)]
mod tests {
    // Not `super::*`: it would bring gpui's `test` attribute macro over `#[test]`.
    use super::{Body, MESSAGE_GONE, More, UNKNOWN_NAME, body, header_text, reacted_at, row_text};
    use crate::format::Clock;
    use crate::store::ReactorsState;
    use bt_chat_core::model::{ConnectionState, MediaRef, Reactor, UserId};
    use bt_chat_core::reactions::{NEUTRAL_GLYPH, ReactionCode, emoji};

    const DAY_US: i64 = 86_400_000_000;
    const NOW: i64 = 1_800_000_000_000_000 - 1_800_000_000_000_000 % DAY_US + 12 * 3_600_000_000;

    fn clock() -> Clock {
        Clock::fixed(NOW, 0)
    }

    fn reactor(guid: &str, name: Option<&str>, code: i64, at: Option<i64>, me: bool) -> Reactor {
        Reactor {
            user: UserId(guid.into()),
            name: name.map(Into::into),
            avatar: None::<MediaRef>,
            code: ReactionCode(code),
            at,
            me,
        }
    }

    fn ready(rows: Vec<Reactor>) -> ReactorsState {
        ReactorsState::Ready {
            rows,
            skipped: 0,
            oldest: Some(1),
            exhausted: true,
            loading_more: false,
            more_failed: None,
        }
    }

    fn with(
        state: &ReactorsState,
        exhausted: bool,
        loading: bool,
        failed: Option<&str>,
    ) -> ReactorsState {
        let ReactorsState::Ready {
            rows,
            skipped,
            oldest,
            ..
        } = state
        else {
            panic!("not a ready state")
        };
        ReactorsState::Ready {
            rows: rows.clone(),
            skipped: *skipped,
            oldest: *oldest,
            exhausted,
            loading_more: loading,
            more_failed: failed.map(Into::into),
        }
    }

    fn more_of(body: Body<'_>) -> More<'_> {
        match body {
            Body::Ready { more, .. } => more,
            other => panic!("not ready: {other:?}"),
        }
    }

    #[test]
    fn body_follows_the_state() {
        let online = ConnectionState::Online;
        assert_eq!(
            body(&ReactorsState::Loading, online, Some(3)),
            Body::Loading
        );
        let failed = ReactorsState::Failed {
            reason: "отказ".into(),
            offline: false,
        };
        assert_eq!(body(&failed, online, None), Body::Failed("отказ"));
        let rows = vec![reactor("a", Some("А"), 1, None, false)];
        let state = ready(rows.clone());
        assert_eq!(
            body(&state, online, Some(5)),
            Body::Ready {
                rows: &rows,
                shown: 1,
                total: Some(5),
                skipped: 0,
                more: More::Hidden,
            }
        );
    }

    #[test]
    fn a_deleted_message_is_gone_and_not_a_failure_to_retry() {
        let gone = ReactorsState::Failed {
            reason: MESSAGE_GONE.into(),
            offline: false,
        };
        for connection in [ConnectionState::Online, ConnectionState::Offline] {
            assert_eq!(body(&gone, connection, Some(2)), Body::Gone);
        }
    }

    #[test]
    fn a_list_is_offline_unless_the_connection_is_online() {
        let state = ready(vec![reactor("a", Some("А"), 1, None, false)]);
        for connection in [ConnectionState::Offline, ConnectionState::Connecting] {
            assert_eq!(body(&state, connection, Some(1)), Body::Offline);
        }
        assert_ne!(
            body(&state, ConnectionState::Online, Some(1)),
            Body::Offline
        );
        // A loading list keeps saying it is loading; the engine answers an offline request at once.
        assert_eq!(
            body(&ReactorsState::Loading, ConnectionState::Offline, None),
            Body::Loading
        );
    }

    #[test]
    fn an_offline_failure_is_offline_whatever_the_connection() {
        let failed = ReactorsState::Failed {
            reason: "Нет соединения".into(),
            offline: true,
        };
        for connection in [ConnectionState::Online, ConnectionState::Offline] {
            assert_eq!(body(&failed, connection, None), Body::Offline);
        }
        let refused = ReactorsState::Failed {
            reason: "r".into(),
            offline: false,
        };
        assert_eq!(
            body(&refused, ConnectionState::Offline, None),
            Body::Failed("r")
        );
    }

    #[test]
    fn more_follows_exhausted_loading_more_and_more_failed() {
        let base = ready(vec![reactor("a", Some("А"), 1, None, false)]);
        fn more(state: &ReactorsState) -> More<'_> {
            more_of(body(state, ConnectionState::Online, None))
        }
        assert_eq!(more(&with(&base, true, false, None)), More::Hidden);
        assert_eq!(more(&with(&base, false, false, None)), More::Offer);
        assert_eq!(more(&with(&base, false, true, None)), More::Loading);
        assert_eq!(
            more(&with(&base, false, false, Some("сбой"))),
            More::Retry("сбой")
        );
    }

    #[test]
    fn more_never_follows_the_known_total() {
        let rows: Vec<Reactor> = (0..3)
            .map(|i| reactor(&format!("g{i}"), Some("А"), 1, None, false))
            .collect();
        let exhausted = ready(rows.clone());
        let open = with(&exhausted, false, false, None);
        for total in [None, Some(0), Some(2), Some(3), Some(100)] {
            // The server has no more: no row, however many reactions the message is known to have.
            assert_eq!(
                more_of(body(&exhausted, ConnectionState::Online, total)),
                More::Hidden,
                "total {total:?}"
            );
            // The server has more: offered, however few the message is known to have.
            assert_eq!(
                more_of(body(&open, ConnectionState::Online, total)),
                More::Offer,
                "total {total:?}"
            );
        }
    }

    #[test]
    fn the_header_rules() {
        // Complete: nothing to declare, also with an unknown total.
        assert_eq!(header_text(30, 0, Some(30)), None);
        assert_eq!(header_text(30, 0, None), None);
        // The count lags below the list: no shortfall.
        assert_eq!(header_text(30, 0, Some(12)), None);
        // Partial.
        assert_eq!(header_text(30, 0, Some(45)).as_deref(), Some("30 из 45"));
        // Entries without identity are accounted for, and said.
        assert_eq!(
            header_text(28, 2, Some(30)).as_deref(),
            Some("2 без данных")
        );
        assert_eq!(header_text(5, 2, None).as_deref(), Some("2 без данных"));
        assert_eq!(
            header_text(28, 2, Some(45)).as_deref(),
            Some("28 из 45 · 2 без данных")
        );
    }

    #[test]
    fn row_content() {
        let at = NOW - 3_600_000_000;
        let known = reactor("a", Some("Алиса"), 1, Some(at), true);
        let text = row_text(&known, clock());
        assert_eq!(text.name, "Алиса");
        assert!(text.own);
        assert_eq!(text.glyph, emoji(ReactionCode(1)).unwrap_or(NEUTRAL_GLYPH));
        assert_eq!(text.when.as_deref(), Some("11:00"));

        let other = row_text(&reactor("b", Some("Боб"), 1, None, false), clock());
        assert!(!other.own);
        assert_eq!(other.when, None);
    }

    #[test]
    fn a_missing_or_blank_name_is_the_placeholder() {
        for name in [None, Some(""), Some("  ")] {
            let text = row_text(&reactor("guid-1", name, 1, None, false), clock());
            assert_eq!(text.name, UNKNOWN_NAME);
            assert!(!text.name.contains("guid-1"));
        }
    }

    #[test]
    fn an_unknown_type_has_the_neutral_glyph_and_stays() {
        let code = -987_654;
        assert_eq!(emoji(ReactionCode(code)), None);
        let text = row_text(&reactor("a", Some("А"), code, None, false), clock());
        assert_eq!(text.glyph, NEUTRAL_GLYPH);
    }

    #[test]
    fn a_known_type_shows_its_emoji() {
        let known = bt_chat_core::reactions::KNOWN[0];
        let text = row_text(&reactor("a", Some("А"), known.code.0, None, false), clock());
        assert_eq!(text.glyph, known.emoji);
    }

    #[test]
    fn the_time_is_the_clock_today_and_the_day_otherwise() {
        let c = clock();
        assert_eq!(reacted_at(NOW - 3_600_000_000, c), "11:00");
        assert_eq!(reacted_at(NOW - DAY_US, c), "Вчера, 12:00");
    }
}
