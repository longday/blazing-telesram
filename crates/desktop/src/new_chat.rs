//! The new chat dialog: pick a person the client already knows, review who the chat will be with, create it.
//!
//! The people on offer are a pure function of the store (`candidates`): the other persons of the account's
//! private chats, named by the chat title, plus the members of the chat whose info panel is open, never the
//! current user. A person who already has a private chat with the account is not created again: choosing
//! them opens that chat (`existing_chat`). Nothing here sends a creation by itself: only the "Создать"
//! button does, and no key (Enter included) reaches it; the field is a single-line input whose Enter is
//! never handled.

use crate::desktop::Desktop;
use crate::input::TextInput;
use crate::store::{ChatFilter, ChatStore, CreationState, MembersState, ProfileState};
use crate::theme::*;
use crate::widgets::{avatar, button};
use bt_chat_core::model::{ChatId, ChatKind, MediaRef, UserId};
use gpui::{prelude::*, *};
use std::collections::HashSet;
use std::ops::Range;

const ROW_HEIGHT: f32 = 48.;
const LIST_HEIGHT: f32 = 240.;

/// A person the dialog offers.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub user: UserId,
    pub name: String,
    pub avatar: Option<MediaRef>,
}

/// The open dialog: the person filter field and the person chosen so far (kept across failures).
pub struct NewChat {
    pub search: Entity<TextInput>,
    pub chosen: Option<Candidate>,
    _repaint: Subscription,
}

impl NewChat {
    pub fn new(cx: &mut Context<Desktop>) -> Self {
        let search = cx.new(|cx| TextInput::single_line("Поиск людей", cx));
        let repaint = cx.observe(&search, |_, _, cx| cx.notify());
        Self { search, chosen: None, _repaint: repaint }
    }
}

/// The people on offer, sorted by name: distinct, never the current user. The current user is known from
/// the loaded profile and from the `me` flag of any loaded member list.
pub fn candidates(store: &ChatStore, panel: Option<&ChatId>) -> Vec<Candidate> {
    let chats = store.visible_chats("", ChatFilter::All);
    let mut me: HashSet<&UserId> = HashSet::new();
    if let Some(ProfileState::Ready(profile)) = store.profile_state() {
        me.insert(&profile.user);
    }
    for chat in &chats {
        if let Some(MembersState::Ready(members)) = store.members_state(&chat.id) {
            me.extend(members.users.iter().filter(|m| m.me).map(|m| &m.user));
        }
    }
    let mut seen: HashSet<UserId> = HashSet::new();
    let mut out = Vec::new();
    for chat in &chats {
        let Some(partner) = chat.partner.as_ref().filter(|_| chat.kind == ChatKind::Private) else { continue };
        if !me.contains(partner) && seen.insert(partner.clone()) {
            out.push(Candidate { user: partner.clone(), name: chat.title.clone(), avatar: chat.avatar.clone() });
        }
    }
    if let Some(MembersState::Ready(members)) = panel.and_then(|chat| store.members_state(chat)) {
        for member in &members.users {
            if !member.me && !me.contains(&member.user) && seen.insert(member.user.clone()) {
                out.push(Candidate {
                    user: member.user.clone(),
                    name: member.name.clone().unwrap_or_else(|| "Без имени".into()),
                    avatar: member.avatar.clone(),
                });
            }
        }
    }
    out.sort_by_cached_key(|c| c.name.to_lowercase());
    out
}

/// The candidates whose name contains the trimmed query, case-insensitively; all of them for a blank query.
pub fn matching<'a>(all: &'a [Candidate], query: &str) -> Vec<&'a Candidate> {
    let query = query.trim().to_lowercase();
    all.iter().filter(|c| query.is_empty() || c.name.to_lowercase().contains(&query)).collect()
}

/// The account's private chat with `user`, if the list holds one.
pub fn existing_chat(store: &ChatStore, user: &UserId) -> Option<ChatId> {
    store
        .visible_chats("", ChatFilter::All)
        .into_iter()
        .find(|c| c.kind == ChatKind::Private && c.partner.as_ref() == Some(user))
        .map(|c| c.id.clone())
}

/// What the dialog says about the creation under the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    Nothing,
    /// A request is out; "Создать" is disabled.
    Sending,
    /// Nothing was created; the text is the reason (also the offline refusal).
    Failed(String),
    /// The request was sent and its result is not known; the chat may exist.
    Unknown(String),
}

pub fn notice(state: Option<&CreationState>) -> Notice {
    match state {
        None => Notice::Nothing,
        Some(CreationState::InFlight) => Notice::Sending,
        Some(CreationState::Failed { reason, unknown: false }) => Notice::Failed(reason.clone()),
        Some(CreationState::Failed { reason, unknown: true }) => Notice::Unknown(reason.clone()),
    }
}

/// "Создать" works with a person chosen and no creation in flight.
pub fn can_create(chosen: bool, state: Option<&CreationState>) -> bool {
    chosen && state != Some(&CreationState::InFlight)
}

fn candidate_row(store: &ChatStore, candidate: &Candidate, chosen: bool, cx: &Context<Desktop>) -> Stateful<Div> {
    let tag = format!("candidate-{}", candidate.user.0);
    let row_tag = tag.clone();
    let user = candidate.user.clone();
    div()
        .id(SharedString::from(tag.clone()))
        .debug_selector(move || row_tag)
        .h(px(ROW_HEIGHT))
        .w_full()
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(12.))
        .rounded(px(8.))
        .cursor_pointer()
        .bg(rgb(if chosen { ROW_SELECTED } else { PANEL }))
        .hover(|s| s.bg(rgb(if chosen { ROW_SELECTED } else { ROW_HOVER })))
        .child(avatar(store, candidate.avatar.as_ref(), &candidate.user.0, &candidate.name, 36., format!("{tag}-avatar")))
        .child(div().flex_1().min_w_0().truncate().child(candidate.name.clone()))
        .on_click(cx.listener(move |this, _, _, cx| this.choose_person(user.clone(), cx)))
}

fn note(selector: &'static str, text: impl Into<SharedString>) -> Div {
    div().debug_selector(move || selector.into()).text_color(rgb(MUTED)).child(text.into())
}

/// The modal. Debug selectors: `new-chat-view`, `new-chat-search`, `candidate-<guid>` (+ `-avatar`),
/// `new-chat-empty`, `new-chat-nomatch`, `new-chat-participant` (+ `-avatar`), `new-chat-sending`,
/// `new-chat-error`, `new-chat-unknown`, `new-chat-cancel`, `new-chat-create`.
pub fn render(state: &NewChat, panel: Option<ChatId>, store: &Entity<ChatStore>, cx: &mut Context<Desktop>) -> Div {
    let query = state.search.read(cx).content.to_string();
    let (all, creation) = {
        let store = store.read(cx);
        (candidates(store, panel.as_ref()), store.creation_state().cloned())
    };
    let shown = matching(&all, &query).len();
    let notice = notice(creation.as_ref());
    let sending = notice == Notice::Sending;
    let enabled = can_create(state.chosen.is_some(), creation.as_ref());

    let list = if all.is_empty() {
        note("new-chat-empty", "Нет известных людей").into_any_element()
    } else if shown == 0 {
        note("new-chat-nomatch", "Никого не найдено").into_any_element()
    } else {
        let (store, search) = (store.clone(), state.search.clone());
        div()
            .h(px(LIST_HEIGHT))
            .child(
                uniform_list(
                    "new-chat-people",
                    shown,
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        let query = search.read(cx).content.to_string();
                        let chosen = this.new_chat_choice().map(|c| c.user.clone());
                        let store = store.read(cx);
                        let all = candidates(store, this.info_chat().as_ref());
                        let rows = matching(&all, &query);
                        range
                            .filter_map(|ix| rows.get(ix))
                            .map(|c| candidate_row(store, c, chosen.as_ref() == Some(&c.user), cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .size_full(),
            )
            .into_any_element()
    };

    let participant = state.chosen.as_ref().map(|chosen| {
        let store = store.read(cx);
        div()
            .debug_selector(|| "new-chat-participant".into())
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(div().text_size(px(13.)).text_color(rgb(MUTED)).child("Личный чат с"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(avatar(
                        store,
                        chosen.avatar.as_ref(),
                        &chosen.user.0,
                        &chosen.name,
                        32.,
                        "new-chat-participant-avatar".into(),
                    ))
                    .child(div().font_weight(FontWeight::MEDIUM).truncate().child(chosen.name.clone())),
            )
    });

    let message = match notice {
        Notice::Nothing => None,
        Notice::Sending => Some(note("new-chat-sending", "Создаём чат…")),
        Notice::Failed(reason) => Some(
            div().debug_selector(|| "new-chat-error".into()).text_color(rgb(LOG_ERROR)).child(reason),
        ),
        Notice::Unknown(reason) => Some(
            div()
                .debug_selector(|| "new-chat-unknown".into())
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_color(rgb(LOG_WARN)).child(reason))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(MUTED))
                        .child("Список чатов обновлён. Нажмите «Создать», чтобы попробовать ещё раз."),
                ),
        ),
    };

    let cancel = button("new-chat-cancel", "Отмена")
        .debug_selector(|| "new-chat-cancel".into())
        .when(sending, |b| b.text_color(rgb(MUTED)).cursor_default())
        .when(!sending, |b| b.on_click(cx.listener(|this, _, _, cx| this.close_new_chat(cx))));
    let create = button("new-chat-create", "Создать")
        .debug_selector(|| "new-chat-create".into())
        .when(!enabled, |b| b.text_color(rgb(MUTED)).cursor_default())
        .when(enabled, |b| {
            b.text_color(rgb(ACCENT)).on_click(cx.listener(|this, _, _, cx| this.confirm_new_chat(cx)))
        });

    div()
        .debug_selector(|| "new-chat-view".into())
        .absolute()
        .size_full()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x000000aa))
        .child(
            div()
                .w(px(420.))
                .p(px(20.))
                .rounded(px(12.))
                .bg(rgb(PANEL))
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(div().text_size(px(17.)).font_weight(FontWeight::MEDIUM).child("Новый чат"))
                .child(
                    div()
                        .debug_selector(|| "new-chat-search".into())
                        .px(px(12.))
                        .py(px(7.))
                        .rounded(px(20.))
                        .bg(rgb(FIELD))
                        .child(state.search.clone()),
                )
                .child(list)
                .children(participant)
                .children(message)
                .child(div().flex().justify_end().gap(px(8.)).child(cancel).child(create)),
        )
}
