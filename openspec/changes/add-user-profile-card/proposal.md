# Proposal

## Why

Clicking another person's avatar in a chat does nothing, and nowhere in the client can the user see who that person is. Today only the account's own profile exists (account menu «Профиль», `profile.rs`, `chat-members`) and the chat info panel lists members by avatar and name only. The web client and Telegram Desktop open a card for any person; the data to fill one is mostly already on the wire the client reads: the member reply and the `get_users_data` reply carry position, department, email and a nickname (see `design.md` "Grounding"). This is a read-only slice: no editing, no calls, no presence claims.

## What Changes

- A read-only **user card**, a modal like the own profile, for any person, opened by: clicking the avatar or the author name of a message in a group chat's timeline; clicking the avatar/name part of a member row in the chat info panel; clicking the avatar in the header of a private chat (the partner). The card of the current account is the same card without «Написать»; the account menu's «Профиль» view stays as it is.
- The card shows what the server tells about that user, verbatim and only when present: name, avatar (or initials), status text, position, department, email, nickname. It shows no presence («в сети», last seen), no phone, no gender and no internal flags; a deleted account is shown as deleted.
- Actions: «Написать» (the existing flow of `chat-creation`: an existing private chat is opened at once, otherwise the confirmation dialog; the card itself writes nothing), «Копировать почту» and «Копировать ID» (the user's guid) to the system clipboard.
- Data source: one new read per card, composed from two methods **already** on the read allowlist: `get_users_data` for the record (a richer parser next to the existing name/avatar one) and, if research R1 confirms it answers for another guid, `whoami { Guid }` for the status text. A new wire method is added only if R1 proves the web client needs one and it is a read; `ALLOWED_METHODS` tests are then extended, not loosened.
- Research first (task 0, live, reads only): key names and value kinds of another user's record and of `whoami` for another guid, how the web client fills its own card, and what an unknown or deleted guid returns. Its results fix which lines of the card exist; no value (name, email, guid) is recorded anywhere.
- Privacy by construction: the card is held in memory only (never in the offline cache, never in the shared name/avatar user cache), asked again at every opening, forgotten on close, sign-out and account switch; no name, email, guid, status or clipboard text reaches the log (counts and outcomes only, as members and reactors do).

## Capabilities

### New Capabilities
- `user-card`: the read-only card of another person: where it opens from, what it shows and never shows, loading/failed/offline states, copy actions, «Написать», privacy and the read-only guarantee.

### Modified Capabilities
<!-- None. The card adds click targets to surfaces owned by `chat-members` (info panel rows, which is not yet in the main specs while its change is being archived) and `message-history` (timeline), but changes none of their requirement texts: every new behaviour is a requirement of `user-card`, so this change merges and archives in any order relative to `add-chat-members-and-creation`. -->

## Impact

- `crates/yandex-proto`: `wire.rs` (`WireUserCard`, `parse_user_card`, next to `parse_users`; `parse_profile` is reused for the status), `registry.rs` and `client.rs` (`Client::user_card(guid)` over `get_users_data`, plus `Client::whoami_of(guid)` if R1 says it answers), `tests/wire_fixtures.rs` and a sanitised live fixture; `bin/bt-probe.rs` (`user-keys <guid>`, shapes only, task 0.2). `request.rs`/`write.rs`: no change unless R1 finds a needed read method; their exact-list tests then follow.
- `crates/chat-core`: `model.rs` (`UserCard`, `Command::LoadUserCard`, `ClientEvent::UserCard` / `UserCardFailed`), `backend.rs` (`user_card`), `client.rs` (engine: offline answers at once, one load per user, never cached), `fixtures.rs` and every `Backend` fake.
- `crates/desktop`: `yandex_backend.rs` (mapping, no log of values), `store.rs` (`UserCardState`, stale-after-reconnect rule as for members), new `user_card.rs` (view), `desktop.rs` (open/close/retry, header avatar, modal layering), `timeline.rs` (avatar and author name become click targets), `info_panel.rs` (row click), `tests.rs`.
- **File overlaps with open work** (details and rule 1 handling in `design.md` D10 and `DEPENDENCIES.md` section UC): `timeline.rs` with `add-text-selection-and-quotes` (proposal in progress on `propose/select`); `desktop.rs`, `store.rs`, chat-core `model.rs` and `client.rs` with `add-notifications-and-badge` NB-N (1.2, 2.2, 4.2, 6.2); `bin/bt-probe.rs` with NB 0.2; `tests.rs` with every unit that appends a section.
- Risk: the shape of another user's record is known only partly (members, `create_private_chat` and search replies; `get_users_data` is observed to carry `contacts`); task 0 closes that before any wire code. «Написать» for a person without a private chat depends on `create_private_chat`, which CM implemented but never verified live (CM 5.1/5.3 are blocked on a target; only the «existing chat opens» path was seen live); this change adds no write of its own and inherits that state.

## Non-goals

- Editing anything (profile, status, contacts), blocking, muting, calling, sharing a contact, adding to a group, showing shared media or common chats.
- Presence: «в сети», last seen, typing in the card. The client sends no heartbeat and no presence request (`request.rs` module doc), and `is_connect` has no known meaning.
- Phone numbers (not observed; `phone_id` is an opaque id), gender, organization ids, the raw guid as text.
- Other places a person appears: reactors list rows, search people hits, reply and forward quotes (names only, no guid), mentions, the new-chat dialog. Each can open the same card later (follow-up).
- Any request that changes server state, and any new live write.

## Follow-ups

- The card from the reactors list, search people hits and the new-chat dialog (the card state is keyed by guid, so only the click targets are missing).
- Position/department of every organisation of a person with several (this change shows one, see Decision 3).
- Presence and last seen, once a read exists whose meaning is known (research R1 records only whether such a key exists).
- Shared media, common groups, mute/block (a separate change with its own live research).

## Decisions on the Open Questions

All questions below were resolved autonomously by taking the recommended answer; `design.md` holds the mechanics and the unresolved items are listed in "Open Questions" there and in the hand-off.

1. **What does the card show?** Decision: name, avatar, status text, position, department, email, nickname, each only when the server sent a non-blank value; a deleted account says so and shows nothing else. Why: all are directory facts the server already sends to every client in member lists; none is interpreted. Phone, gender and flags are left out: unobserved, opaque or irrelevant.
2. **Presence / last seen?** Decision: no. Why: no read with a known meaning (`is_connect` is unexplained; the status events were judged unsafe in `add-chat-members-and-creation`), and a wrong «в сети» is worse than none. R1 only records whether such a key exists, for the follow-up.
3. **Several organisations?** Decision: one position/department line pair: `employee_info` (the server's current one), else the first of `employees_info`. Why: the record carries both and the semantic of the list order is unobserved; the card must not pick by guessing among many.
4. **Entry points.** Decision: group-chat timeline avatar and author name, info-panel row (avatar and name part; «Написать» keeps its own click), private-chat header avatar (the rest of the header still toggles the info panel, so the `chat-members` behaviour is unchanged). The current account opens the same card, without «Написать», instead of a redirect to the profile view: the click sites do not all know the own guid (the store does not hold it; the header would have to wait for a profile read), while the backend does and flags the card. Private-chat timelines have neither avatar nor name today, so the header is their only entry. Why: exactly the surfaces named in the request, with no change to an archived spec's behaviour; making the whole header open the card in a private chat is a one-line follow-up if wanted.
5. **«Написать».** Decision: reuse `Desktop::open_new_chat(Some(user))` unchanged; the card closes first. Why: one creation path with one confirmation. The creation call (`create_private_chat`) is live-unverified (CM 5.1/5.3); an existing private chat is the common case and was verified live.
6. **Copy actions.** Decision: «Копировать почту» (only when an email is shown) and «Копировать ID» (guid; the guid itself is not displayed). Why: the guid is useful for support and probes but is noise on screen. The copied text never reaches the log.
7. **Source of the data / a new wire method?** Decision: existing reads only (`get_users_data`, `whoami { Guid }` if it answers); a new method only on R1 evidence, a read by its own semantics, as a new `ReadRequest` variant with the exact-list tests extended. Why: the closed allowlist is a safety property of this client.
8. **Stored anywhere?** Decision: no. Memory only, per opening, never in `offline-cache`, never in the adapter's shared `users` map (that map outlives the card and holds only name/avatar/uid). Why: contact data of other people must not persist on this machine beyond what the user is looking at.
9. **When the details cannot be read** (offline, request failed, user not found). Decision: the card still shows the name and avatar the opening surface already had (they are what the chat shows anyway) and replaces the detail lines by the reason, with a retry; no detail from an earlier opening is ever shown. Why: the same rule as the own profile, without blanking the one thing the user clicked on.
