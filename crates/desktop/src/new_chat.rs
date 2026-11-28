//! The new chat dialog: pick who the chat is with, review it, create it.
//!
//! Two variants share the dialog (`Mode`). Private: choose one known person; someone who already has a private
//! chat with the account is not created again, choosing them opens that chat (`existing_chat`). Group: a title
//! and any number of known people (a click toggles, "✕" takes one out); no existing chat is looked for, a group
//! is always new. "Создать" is enabled for a group only with a usable title (`title_ok`) and at least one
//! participant.
//!
//! The people on offer are a pure function of the store (`candidates`): the other persons of the account's
//! private chats, named by the chat title, plus the members of the chat whose info panel is open, never the
//! current user. Nothing here sends a creation by itself: only the "Создать" button does, and no key (Enter
//! included) reaches it; the search and title fields are single-line inputs whose Enter is never handled.

use crate::desktop::Desktop;
use crate::input::TextInput;
use crate::store::{ChatFilter, ChatStore, CreationState, MembersState, ProfileState};
use crate::theme::*;
use crate::widgets::{avatar, button};
use bt_chat_core::model::{ChatId, ChatKind, ChatSummary, MediaRef, UserId};
use gpui::{prelude::*, *};
use std::collections::HashSet;
use std::ops::Range;
use yandex_proto::manage::MAX_CHAT_TITLE;

const ROW_HEIGHT: f32 = 48.;
const LIST_HEIGHT: f32 = 240.;
/// The list is shorter in the group variant, which also shows the title and the participants.
const GROUP_LIST_HEIGHT: f32 = 144.;
const MEMBERS_HEIGHT: f32 = 108.;
/// Space kept between the card and the window edge.
const CARD_MARGIN: f32 = 16.;

/// A person the dialog offers.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub user: UserId,
    pub name: String,
    pub avatar: Option<MediaRef>,
}

/// What the dialog creates: a private chat with one person, or a group with a title and several people.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Private,
    Group,
}

/// The open dialog: the filter field, the group title field, what is chosen so far (kept across
/// failures and mode switches): one person for a private chat, the participants for a group.
pub struct NewChat {
    pub search: Entity<TextInput>,
    pub title: Entity<TextInput>,
    pub mode: Mode,
    pub chosen: Option<Candidate>,
    /// The group's participants in the order they were picked.
    pub members: Vec<Candidate>,
    _repaint: Vec<Subscription>,
}

impl NewChat {
    pub fn new(cx: &mut Context<Desktop>) -> Self {
        let search = cx.new(|cx| TextInput::single_line("Поиск людей", cx));
        let title = cx.new(|cx| TextInput::single_line("Название группы", cx));
        let repaint = vec![
            cx.observe(&search, |_, _, cx| cx.notify()),
            cx.observe(&title, |_, _, cx| cx.notify()),
        ];
        Self {
            search,
            title,
            mode: Mode::Private,
            chosen: None,
            members: Vec::new(),
            _repaint: repaint,
        }
    }

    /// Whether `user` is the chosen person (private) or one of the participants (group).
    pub fn is_selected(&self, user: &UserId) -> bool {
        match self.mode {
            Mode::Private => self.chosen.as_ref().is_some_and(|c| c.user == *user),
            Mode::Group => self.members.iter().any(|m| m.user == *user),
        }
    }

    /// Adds the person to the group, or takes them out when they are in.
    pub fn toggle_member(&mut self, person: Candidate) {
        match self.members.iter().position(|m| m.user == person.user) {
            Some(ix) => {
                self.members.remove(ix);
            }
            None => self.members.push(person),
        }
    }
}

/// The ids of the current user as far as the store knows them: the loaded profile and the `me` flag of any
/// loaded member list. Empty until one of them has loaded.
fn own_ids<'a>(store: &'a ChatStore, chats: &[&'a ChatSummary]) -> HashSet<&'a UserId> {
    let mut me: HashSet<&UserId> = HashSet::new();
    if let Some(ProfileState::Ready(profile)) = store.profile_state() {
        me.insert(&profile.user);
    }
    for chat in chats {
        if let Some(MembersState::Ready(members)) = store.members_state(&chat.id) {
            me.extend(members.users.iter().filter(|m| m.me).map(|m| &m.user));
        }
    }
    me
}

/// Whether the current user is known (see `own_ids`). Until then the partners of private chats are not
/// offered: the self chat would show up as a person to write to. Members of the open panel carry their own
/// `me` flag and do not wait for it.
pub fn identity_known(store: &ChatStore) -> bool {
    !own_ids(store, &store.visible_chats("", ChatFilter::All)).is_empty()
}

/// Whether `user` is the current user as far as the store knows (see `own_ids`); false until the identity
/// is known.
pub fn is_current_user(store: &ChatStore, user: &UserId) -> bool {
    own_ids(store, &store.visible_chats("", ChatFilter::All)).contains(user)
}

/// The people on offer, sorted by name: distinct, never the current user. The partners of private chats
/// wait for the identity (`identity_known`).
pub fn candidates(store: &ChatStore, panel: Option<&ChatId>) -> Vec<Candidate> {
    let chats = store.visible_chats("", ChatFilter::All);
    let me = own_ids(store, &chats);
    let mut seen: HashSet<UserId> = HashSet::new();
    let mut out = Vec::new();
    for chat in chats.iter().filter(|_| !me.is_empty()) {
        let Some(partner) = chat
            .partner
            .as_ref()
            .filter(|_| chat.kind == ChatKind::Private)
        else {
            continue;
        };
        if !me.contains(partner) && seen.insert(partner.clone()) {
            out.push(Candidate {
                user: partner.clone(),
                name: chat.title.clone(),
                avatar: chat.avatar.clone(),
            });
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
    all.iter()
        .filter(|c| query.is_empty() || c.name.to_lowercase().contains(&query))
        .collect()
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
    /// The request was sent, its result is not known and the refreshed list did not hold the chat.
    Unknown(String),
    /// The chat may exist and the chat list could not be read to tell; the text is the reason.
    Unverified(String),
}

pub fn notice(state: Option<&CreationState>) -> Notice {
    match state {
        None => Notice::Nothing,
        Some(CreationState::InFlight) => Notice::Sending,
        Some(CreationState::Failed {
            reason,
            unknown: false,
        }) => Notice::Failed(reason.clone()),
        Some(CreationState::Failed {
            reason,
            unknown: true,
        }) => Notice::Unknown(reason.clone()),
        Some(CreationState::Unverified { reason }) => Notice::Unverified(reason.clone()),
    }
}

/// "Создать" works with a person chosen and no creation in flight.
pub fn can_create(chosen: bool, state: Option<&CreationState>) -> bool {
    chosen && state != Some(&CreationState::InFlight)
}

/// The length of a group title in characters, as the server's limit counts it.
fn title_chars(title: &str) -> usize {
    title.chars().count()
}

/// A group title the server takes: not blank and no longer than the web client lets a user type.
pub fn title_ok(title: &str) -> bool {
    !title.trim().is_empty() && title_chars(title) <= MAX_CHAT_TITLE
}

/// "Создать" for a group works with a usable title, at least one participant and no creation in flight.
pub fn can_create_group(title: &str, members: usize, state: Option<&CreationState>) -> bool {
    title_ok(title) && members > 0 && state != Some(&CreationState::InFlight)
}

/// `multi` is the group variant: a mark shows who is in, a second click takes them out.
fn candidate_row(
    store: &ChatStore,
    candidate: &Candidate,
    chosen: bool,
    multi: bool,
    cx: &Context<Desktop>,
) -> Stateful<Div> {
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
        .child(avatar(
            store,
            candidate.avatar.as_ref(),
            &candidate.user.0,
            &candidate.name,
            36.,
            format!("{tag}-avatar"),
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(candidate.name.clone()),
        )
        .when(multi && chosen, |row| {
            row.child(
                div()
                    .debug_selector(move || format!("{tag}-check"))
                    .text_color(rgb(ACCENT))
                    .child("✓"),
            )
        })
        .on_click(cx.listener(move |this, _, _, cx| this.choose_person(user.clone(), cx)))
}

fn note(selector: &'static str, text: impl Into<SharedString>) -> Div {
    div()
        .debug_selector(move || selector.into())
        .text_color(rgb(MUTED))
        .child(text.into())
}

/// The modal. Debug selectors: `new-chat-view`, `new-chat-mode-private` / `new-chat-mode-group`,
/// `new-chat-title` (group), `new-chat-search`, `candidate-<guid>` (+ `-avatar`, group: `-check` when
/// selected), `new-chat-empty`, `new-chat-identity`, `new-chat-nomatch`, `new-chat-participant` (+ `-avatar`;
/// private), `new-chat-members` with `new-chat-members-empty`, `new-chat-member-<guid>` (+ `-avatar`) and
/// `new-chat-remove-<guid>` (group), `new-chat-title-long`, `new-chat-sending`, `new-chat-error`,
/// `new-chat-unknown`, `new-chat-unverified`, `new-chat-cancel`, `new-chat-create`; `new-chat-card` is the
/// panel (never taller than the window) and `new-chat-body` its scrolling middle.
pub fn render(
    state: &NewChat,
    panel: Option<ChatId>,
    store: &Entity<ChatStore>,
    cx: &mut Context<Desktop>,
) -> Div {
    let query = state.search.read(cx).content.to_string();
    let title = state.title.read(cx).content.to_string();
    let group = state.mode == Mode::Group;
    let (all, creation, identity) = {
        let store = store.read(cx);
        let identity = match (identity_known(store), store.profile_state()) {
            (true, _) => None,
            (false, Some(ProfileState::Failed { .. })) => Some(
                "Не удалось определить вашу учётную запись. Закройте окно и откройте его снова.",
            ),
            (false, _) => Some("Загрузка…"),
        };
        (
            candidates(store, panel.as_ref()),
            store.creation_state().cloned(),
            identity,
        )
    };
    let shown = matching(&all, &query).len();
    let notice = notice(creation.as_ref());
    let sending = notice == Notice::Sending;
    let enabled = if group {
        can_create_group(&title, state.members.len(), creation.as_ref())
    } else {
        can_create(state.chosen.is_some(), creation.as_ref())
    };

    let list = if let (true, Some(text)) = (all.is_empty(), identity) {
        note("new-chat-identity", text).into_any_element()
    } else if all.is_empty() {
        note("new-chat-empty", "Нет известных людей").into_any_element()
    } else if shown == 0 {
        note("new-chat-nomatch", "Никого не найдено").into_any_element()
    } else {
        let (store, search) = (store.clone(), state.search.clone());
        div()
            .h(px(if group {
                GROUP_LIST_HEIGHT
            } else {
                LIST_HEIGHT
            }))
            .child(
                uniform_list(
                    "new-chat-people",
                    shown,
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        let query = search.read(cx).content.to_string();
                        let multi = this.new_chat_is_group();
                        let store = store.read(cx);
                        let all = candidates(store, this.info_chat().as_ref());
                        let rows = matching(&all, &query);
                        range
                            .filter_map(|ix| rows.get(ix))
                            .map(|c| {
                                let chosen = this.new_chat_selected(&c.user);
                                candidate_row(store, c, chosen, multi, cx)
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .size_full(),
            )
            .into_any_element()
    };

    let participant = state.chosen.as_ref().filter(|_| !group).map(|chosen| {
        let store = store.read(cx);
        div()
            .debug_selector(|| "new-chat-participant".into())
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(rgb(MUTED))
                    .child("Личный чат с"),
            )
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
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(chosen.name.clone()),
                    ),
            )
    });

    // The group's participants, in the order they were picked, each with a control to take them out.
    let members = group.then(|| {
        let store = store.read(cx);
        let rows = state.members.iter().map(|member| {
            let tag = format!("new-chat-member-{}", member.user.0);
            let remove_tag = format!("new-chat-remove-{}", member.user.0);
            let user = member.user.clone();
            div()
                .debug_selector({
                    let tag = tag.clone();
                    move || tag
                })
                .h(px(36.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(avatar(
                    store,
                    member.avatar.as_ref(),
                    &member.user.0,
                    &member.name,
                    28.,
                    format!("{tag}-avatar"),
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(member.name.clone()),
                )
                .child(
                    div()
                        .id(SharedString::from(remove_tag.clone()))
                        .debug_selector(move || remove_tag)
                        .px(px(8.))
                        .text_color(rgb(MUTED))
                        .child("✕")
                        .when(!sending, |b| {
                            b.cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_member(user.clone(), cx)
                                }))
                        }),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(rgb(MUTED))
                    .child(format!("Участники группы: {}", state.members.len())),
            )
            .child(if state.members.is_empty() {
                note("new-chat-members-empty", "Выберите хотя бы одного человека")
                    .into_any_element()
            } else {
                div()
                    .id("new-chat-members")
                    .debug_selector(|| "new-chat-members".into())
                    .max_h(px(MEMBERS_HEIGHT))
                    .overflow_y_scroll()
                    .children(rows)
                    .into_any_element()
            })
    });

    let title_field = group.then(|| {
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .debug_selector(|| "new-chat-title".into())
                    .px(px(12.))
                    .py(px(7.))
                    .rounded(px(20.))
                    .bg(rgb(FIELD))
                    .child(state.title.clone()),
            )
            .when(title_chars(&title) > MAX_CHAT_TITLE, |d| {
                d.child(
                    note(
                        "new-chat-title-long",
                        format!("Название длиннее {MAX_CHAT_TITLE} символов"),
                    )
                    .text_size(px(13.))
                    .text_color(rgb(LOG_ERROR)),
                )
            })
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
        Notice::Unverified(reason) => Some(
            div()
                .debug_selector(|| "new-chat-unverified".into())
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_color(rgb(LOG_WARN)).child(reason))
                .child(
                    div().text_size(px(13.)).text_color(rgb(MUTED)).child(
                        "Список чатов не прочитан. Перед новой попыткой клиент сначала его прочитает и ничего не отправит, пока это не удастся.",
                    ),
                ),
        ),
    };

    let mode_button =
        |mode: Mode, id: &'static str, label: &'static str| {
            let selected = state.mode == mode;
            div()
                .id(id)
                .px(px(14.))
                .py(px(8.))
                .rounded(px(8.))
                .text_size(px(14.))
                .cursor_pointer()
                .bg(rgb(if selected { ROW_SELECTED } else { FIELD }))
                .hover(|s| s.bg(rgb(if selected { ROW_SELECTED } else { ROW_HOVER })))
                .child(label)
                .debug_selector(move || id.into())
                .when(sending, |b| b.text_color(rgb(MUTED)).cursor_default())
                .when(!sending, |b| {
                    b.on_click(cx.listener(move |this, _, window, cx| {
                        this.set_new_chat_mode(mode, window, cx)
                    }))
                })
        };
    let modes = div()
        .flex()
        .gap(px(8.))
        .child(mode_button(
            Mode::Private,
            "new-chat-mode-private",
            "Личный чат",
        ))
        .child(mode_button(Mode::Group, "new-chat-mode-group", "Группа"));

    let cancel = button("new-chat-cancel", "Отмена")
        .debug_selector(|| "new-chat-cancel".into())
        .when(sending, |b| b.text_color(rgb(MUTED)).cursor_default())
        .when(!sending, |b| {
            b.on_click(cx.listener(|this, _, _, cx| this.close_new_chat(cx)))
        });
    let create = button("new-chat-create", "Создать")
        .debug_selector(|| "new-chat-create".into())
        .when(!enabled, |b| b.text_color(rgb(MUTED)).cursor_default())
        .when(enabled, |b| {
            b.text_color(rgb(ACCENT))
                .on_click(cx.listener(|this, _, _, cx| this.confirm_new_chat(cx)))
        });

    // The card never outgrows the window. The heading, the variant buttons and the input fields (title,
    // search) stay on top and the buttons at the bottom, so a field that has the focus is never scrolled out
    // of sight; the people, participants and notices between them scroll. Without that a group with a few
    // participants is taller than the smallest window and "Создать" is out of reach.
    div()
        .debug_selector(|| "new-chat-view".into())
        .absolute()
        .size_full()
        .p(px(CARD_MARGIN))
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x000000aa))
        .child(
            div()
                .debug_selector(|| "new-chat-card".into())
                .w(px(420.))
                .max_h_full()
                .p(px(20.))
                .rounded(px(12.))
                .bg(rgb(PANEL))
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .child(
                            div()
                                .text_size(px(17.))
                                .font_weight(FontWeight::MEDIUM)
                                .child("Новый чат"),
                        )
                        .child(modes)
                        .children(title_field)
                        .child(
                            div()
                                .debug_selector(|| "new-chat-search".into())
                                .px(px(12.))
                                .py(px(7.))
                                .rounded(px(20.))
                                .bg(rgb(FIELD))
                                .child(state.search.clone()),
                        ),
                )
                .child(
                    div()
                        .id("new-chat-body")
                        .debug_selector(|| "new-chat-body".into())
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .flex_col()
                                .gap(px(12.))
                                .child(list)
                                .children(participant)
                                .children(members)
                                .children(message),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(cancel)
                        .child(create),
                ),
        )
}
