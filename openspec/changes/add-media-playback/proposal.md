# Proposal

## Why

Voice messages and videos already arrive in history, and the client cannot use them: a voice message is the placeholder «[Голосовое сообщение]», a video is an ordinary file card that can only be saved. Roadmap item 2 (media) of `add-read-only-client` ends with "voice/video playback"; `add-media-download` (archived 2026-11-25) split it out on purpose, "after the URLs, the audio/video formats and the local GPUI/macOS support are confirmed" (its `research.md`, section 3). This change is that read-only remainder: the user presses play and hears a voice message in the timeline, presses play on a video and watches it. Nothing is sent, recorded, uploaded or marked on the server.

`research.md` (next to this file) is the single evidence document. It starts with the research tasks (what is unknown and how it is measured, each doable read-only with the app account, none needs the user), then the facts already established.

## What Changes

- **Voice messages become a player in the bubble**, as the mockup draws it (`docs/mockups/1-main.svg`): a round play/pause control, a waveform-style progress track, the time. Playback starts only on a click; the audio is fetched at that click, never before; one voice message plays at a time; it stops on leaving the chat, on sign-out and when the message disappears. Duration is shown from history; progress follows the position.
- **Video attachments** (a `MiscFile` that carries a video preview hint and has a recognised video extension) get a video card with a play control, the duration and the size. Play fetches a **playback copy** with visible progress and cancel, then hands it to the **system player** (QuickTime or the user's default) through `gpui`'s `open_with_system`. The client does not decode or draw video itself (alternatives recorded in `design.md` D8).
- **A playback copy** is a separate local file in the account's media area (never the Downloads folder), fetched through the existing download transfer machinery under a new media kind `Playback`, removed at app start and at sign-out. It is not a download: no «Сохранено», no «Показать в Finder».
- **Voice and video keep the normal Download action** (`attachment-download`): the same one-line action row as a file card under the player, so the user can still save the original. Play and Download are independent.
- `yandex-proto`: decode the voice waveform and the video preview hint; new media targets for the playback URL; `bt-probe download` accepts voice and video messages (needed by the research tasks).
- `chat-core`: `MediaKind::Playback`, `MessageBody::{Voice, Video}`, a playback destination for the download engine with its own transfer slots, the playback directory wiped at start.
- `desktop`: an audio output behind a small trait (the real one chosen by research task R3-R5 under the rule of `design.md` D5), a `Player` entity, the voice bubble and the video card, a `format` helper for `m:ss`, the hand-off in `main.rs`.
- Specs: new `media-playback`; `attachment-download` (two requirements modified: Download is offered on voice and video too, and the one allowed way to open a playback copy); `message-history` («Attachments preview» modified: voice and video).

## Capabilities

### New Capabilities
- `media-playback`: the voice player, the video card and its hand-off to the system player, the playback copy and its lifetime, stopping rules, failure and offline behaviour, privacy.

### Modified Capabilities
- `attachment-download`: MODIFIED «Explicit download action» (voice and video bubbles offer Download; Play is the one other explicit action that fetches an original) and «Authenticated and private transfer» (the "never open a saved file" rule keeps holding for downloads; a playback copy of a recognised video type may be handed to the system player on an explicit Play).
- `message-history`: MODIFIED «Attachments preview» (a voice message is a player, a video is a video card, both in chat-row previews as «Голосовое сообщение» / «Видео»).

`offline-cache`, `chat-list`, `live-updates`, `account-session` are not modified: voice and video bodies are cached like any body, the playback copy is not part of the cache, and sign-out already deletes the account's media directory (a scenario in `media-playback` pins that).

## Impact

- `crates/yandex-proto`: `wire.rs` (`WireBody::Voice.waveform`, new `WireBody::Video`), `media.rs` (`MediaTarget::Playback`; `VideoStream` only if research keeps it), `bin/bt-probe.rs` (`download` for voice/video, `--url`), `tests/wire_fixtures.rs` and fixtures (sanitised live captures).
- `crates/chat-core`: `model.rs` (`MediaKind::Playback`, `MediaRef::playback`, `MessageBody::{Voice, Video}`, `preview_text`), `client.rs` (playback destination, slots, wipe at start), `cache.rs` (`kind_name` arm only), `fixtures.rs`.
- `crates/desktop`: new `player.rs` (trait, state machine, fake for tests, `Player` entity), new `audio.rs` (the real backend), `yandex_backend.rs` (mapping, `download_target`), `store.rs` (`want_media` ignores `Playback`), `timeline.rs` (bubble, card), `desktop.rs` (owns the `Player`, stops it on chat switch and deletion), `main.rs` (backend registration, the `open_with_system` call), `format.rs`, `theme.rs` (only if a colour is missing), `tests.rs`, `Cargo.toml`/`Cargo.lock`.
- New dependency: one audio crate set (the default is `objc2-avf-audio` with `objc2-foundation` features at versions compatible with the pinned `objc2 =0.6.5`); its licence is checked by research task R5 and recorded in `THIRD_PARTY.md` only if it needs a note. The icons are text glyphs, no Telegram graphics, so the `openspec/config.yaml` relicense-to-GPL trigger (Telegram Desktop graphics) is not hit; that trigger is unrelated to the licence of an audio crate.
- Coordination: `DEPENDENCIES.md` rule 1; this change touches `store.rs`, `timeline.rs`, `desktop.rs`, `main.rs`, `tests.rs`, `yandex_backend.rs`, `client.rs`, `model.rs`, `wire.rs`, `bt-probe.rs` like the other open changes; no unmerged task holds them at the time of writing (CM and PIN have only live tasks left), the order is in `tasks.md`.
- Risks: the protocol is unofficial and the audio format and the duration unit are unobserved (research); audio output cannot be heard in the VM (a limit of the hand check); a playback copy of a large video occupies disk until the next start (accepted, D3); the system player is outside the app's control (D8).

## Non-goals

- Uploading or sending media, recording voice, voice-message sending, video calls (`docs/mockups/10-call.svg`), the media browser (`get_media_messages`).
- Transcription: `Voice.Text`/`WasRecognized` are not shown; whether history carries them is only recorded by research R1.
- An inline video player (decoding and drawing frames in the app), video thumbnails or a blurhash placeholder, picture-in-picture.
- Playback speed (1x/1.5x/2x), seeking by clicking the track, a "listened" marker (local or server), autoplay of the next voice message, a mini player that outlives the chat, background playback, media keys, volume control (the system volume is used).
- Playing a file the user saved to Downloads: the client still never opens a saved download (`attachment-download`).
- Video that is not recognised as video by the rule of `design.md` D2 (it stays a file card), animated GIF/"video note" kinds, stickers.

## Open questions

None needs the user. What is unknown is unknown about the server and the platform, and research tasks R1-R6 answer it read-only; every one has a stated default in `research.md` ("What each unknown decides") and a decision rule in `design.md`.

## Decisions taken without asking

Recorded in `design.md` (D1-D11); the ones a reviewer might want to veto: voice plays inline but speed and seeking are out (D6, D10); a playback copy is a separate kind and directory, not a download (D3); the hand-off to the system player is limited to a whitelist of video extensions because `open` on an arbitrary extension can launch an application (D8); the audio backend is picked by a written rule from measured facts, not by taste (D5); playback stops when the chat is left instead of a global mini player (D6); the player is reached through a gpui global so `Desktop::new` keeps its signature (D6).
