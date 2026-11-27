# Design

## Context

`proposal.md` states the motivation, `research.md` the unknowns and the facts. Code this change extends (read on 2026-11-28):

- `yandex-proto/src/wire.rs`: `WireBody::{Voice { file, duration }, File(WireFile)}`; `Plain.MiscFile` -> `File`, `PreviewHint` ignored (`plain_body`, `file_info`).
- `yandex-proto/src/media.rs`: `MediaTarget::{Avatar, Thumbnail, Original}`, `MediaClient::download` (streamed, cookie only for its host).
- `chat-core`: `MediaKind::{Avatar, Thumbnail, File}`, `MessageBody::{Text, Image, Gallery, File, Deleted, Unsupported}`, `Command::Download`, `Engine::start_download`/`run_download` (`downloads: HashMap<MediaRef, Transfer>`, `DOWNLOAD_SLOTS`, `create_partial`/`commit_partial`), `sign_out` removing `media_dir`, `cache::kind_name`.
- `desktop`: `yandex_backend.rs::body` (voice -> `Unsupported`), `download_target`; `store.rs` (`downloads: HashMap<MediaRef, DownloadState>`, `download`, `cancel_download`, `download_state`, `want_media` skips `File`; the `SignedOut`/`SessionExpired` arms); `timeline.rs` (`file_card`, `action_row`, `RowContext`); `viewer.rs::action_line`; `main.rs` (the app-level store subscription, where `StoreEvent::Reveal` calls `cx.reveal_path`).
- `gpui` 0.2.2 (pinned): no audio, a `Surface` for `CVPixelBuffer`, `App::open_with_system` (`open <path>`), test platform `open_with_system` and `reveal_path` are `unimplemented!()` (research R7).

## Goals / Non-Goals

**Goals:** a voice message plays and pauses in its bubble; a video plays in the system player; the original of either can still be downloaded; nothing is fetched, played or sent without a click; contracts below are fixed so protocol, core, adapter, player, views and tests can be written in parallel.

**Non-Goals:** the proposal's non-goals.

## Decisions

### D1. Shape of the change

Four seams, each testable alone: **wire** (decode the fields), **transfer** (fetch a playback copy with the existing download engine), **player** (an audio output behind a trait plus a state machine, no UI), **views** (a bubble and a card that render the player's and the store's state). Video shares the transfer and differs only in what happens after the copy is on disk (D8).

### D2. Wire, model and recognition

```rust
// yandex-proto/src/wire.rs
WireBody::Voice { file: WireFile, duration: Option<u64>, waveform: Option<Vec<u8>> }   // duration stays raw
WireBody::Video(WireVideo)
pub struct WireVideo { pub file: WireFile, pub width: u32, pub height: u32, pub duration_ms: u64 }

// chat-core/src/model.rs
MediaKind::Playback                      // a playback copy of an original; MediaRef.id is the Id2
impl MediaRef { pub fn playback(&self) -> MediaRef }   // {id, kind: Playback}, like original()
MessageBody::Voice { media: MediaRef /*kind File*/, duration_ms: Option<u64>, size: u64,
                     name: Option<String>, waveform: Option<Vec<u8>> }
MessageBody::Video { media: MediaRef /*kind File*/, name: String, size: u64,
                     width: u32, height: u32, duration_ms: u64 }
```

- `Voice.Duration` is stored raw by the wire and converted to `duration_ms` in the adapter by the unit research R3 finds. Until then the adapter treats it as **seconds** ([INFERENCE]: the synthetic fixture pairs `Duration: 12` with a 4 000-byte file, which fits 12 seconds of heavily compressed speech and would be an implausible 12 milliseconds). A missing duration is `None`; the bubble then shows no time until the audio is loaded.
- The waveform is decoded in `wire.rs` as one byte per bar when R1/R3 show the encoding (expected: base64 of bytes); an unknown or malformed form is `None`, never an error. Blocked on R1 for the decoder; the bars fall back to flat (D7).
- **A video is a `MiscFile` whose `PreviewHint.VideoPreview` object is present.** `PreviewHint` absent means an ordinary file, whatever its extension: guessing from names would turn documents and archives into players. The adapter additionally requires the file name's extension to be in the **video whitelist** (D8); otherwise the body stays `MessageBody::File`, the file card as today. So a `MessageBody::Video` always has a playable-looking, safe extension.
- `MessageBody::preview_text`: `Voice` -> «Голосовое сообщение», `Video` -> «Видео» (no brackets, like «Фото»). Chat rows, search hits and reply quotes use it, so none of them shows «[Голосовое сообщение]» any more.
- Cached bodies written before this change are `Unsupported("Голосовое сообщение")` and keep showing the placeholder until the server's copy replaces them (the existing consistency rule of `offline-cache`); nothing migrates. Both new variants are plain serde additions; no SQL migration.
- Forwarded and replied-to originals go through the same `plain_body`, so a voice or video inside a forward decodes; only the quote text uses it (`preview_text`), the player is shown for the message's own body only.

### D3. A playback copy is a separate kind of transfer, not a download to Downloads

Why not the existing paths: `Command::Download` saves into the user's Downloads folder (spec: visible, named, never deleted, `Saved` card); `fetch_media` is buffered with a 25 MiB cap and meant for previews. A playback file is neither: it is a private temporary, it must not appear in Downloads, and a video can be far above 25 MiB.

- `MediaKind::Playback` is the key. `Command::Download { media: <playback ref>, name }` and the existing `ClientEvent::{DownloadProgress, Downloaded, DownloadFailed, DownloadCancelled}` carry it unchanged; the desktop store's `downloads` map keeps it apart from the original's state because the kind differs (`media.original()` vs `media.playback()` are different keys), so "Скачать" and "Воспроизвести" never confuse each other's progress.
- `Engine::start_download` accepts `File` (destination: `config.downloads_dir`, as today) and `Playback` (destination: `media_dir(runtime, account)/playback`, created `0700`, files `0600`). A failure to create it is `DownloadFailure::Destination`. Any other kind is still refused with the existing warning.
- **File name of a copy:** `media_file_name(media)` (kind prefix plus escaped id, already path-safe) followed by the extension of the supplied name **only if** it matches `\.[A-Za-z0-9]{1,8}`; else none. The attachment's own name is never used as a path component for a playback copy (a hostile name cannot matter), and the system player still gets the extension it needs (research R6).
- **Slots:** playback has its own `Semaphore` (2 permits), not `DOWNLOAD_SLOTS`: a 10-second voice message must not wait behind four large user downloads.
- **Lifetime:** the playback directory is removed (best effort, missing is fine) when the engine starts for an account and when it switches account; sign-out already removes `media_dir` recursively; nothing else deletes copies, so within one run a copy is reused by the store (`DownloadState::Saved(path)` plus a `path.is_file()` check at click time; a missing file triggers a new transfer). A run's copies therefore cost disk until the next start; no cap in this change (a risk, below).
- Reuse: an existing `Saved` copy is used also **offline** (the file is local; the store checks it before the connection, unlike `download`'s offline refusal for new transfers). The key is the file's `Id2`, not the name or the type, so a reused copy was possibly written for another message that holds the same file under another name; for a video its extension is therefore checked on the actual path before it is opened (D8, "the check is on the path").
- The copy is not indexed anywhere (no `Cache.media` row, no migration). `cache::kind_name` gains the `"playback"` arm only because the match is exhaustive.
- **Threat model of the playback folder:** the runtime directory is the trusted root. An attacker who can already write inside the app's own media directory is out of scope; a check-then-use race (TOCTOU) against such a writer is an accepted residual. What is guaranteed: the app never follows a symlink it did not create and never leaves a readable window. Every folder below the runtime directory (`media`, the account's folder, the playback folder) is looked at with `symlink_metadata` (no following), by one helper used on all three paths: the cleanup (engine start, `switch_account`) skips, with a warning that names no path, when any of them is a symlink or not a folder; a transfer is refused with `Destination` in the same case, before anything below the runtime directory is created, and looks again right before the partial file is created and right before it is finalised; an existing playback folder with a mode other than `0700` is set to `0700`; the partial file is created with mode `0600` at open (`create_new`), not changed afterwards.

### D4. URL of the playback copy

`MediaTarget::Playback { file_id }` = `files_base/file_shortterm/{id}` with **no query** (the notes' `voicePlayUrl`; the same host, session cookie and redirect handling as `Original`, `safe_path` for the id). Used for voice and video alike. `file_shortterm/{id}?attach=true` (today's `Original`) is the documented fallback if R2 shows the plain URL is not served for some `Source`. `video_streams/{id}` (the notes' `videoStreamsUrl`) is **not** the plan: a stream URL likely returns a manifest that the system player cannot open with our cookie (R2 measures it); `MediaTarget::VideoStream` exists only as a research probe target and is deleted in task 1.2 unless R2 makes it the answer. `adapter::download_target` maps `Playback` -> `MediaTarget::Playback`, `File` -> `Original` as now.

### D5. Audio output: the trait, and the rule that picks the backend

```rust
// desktop/src/player.rs
pub trait AudioOutput {
    /// Opens `path` and returns its length; no sound yet.
    fn open(&mut self, path: &Path) -> Result<Duration, OpenError>;
    fn play(&mut self) -> Result<(), OpenError>;
    fn pause(&mut self);
    fn stop(&mut self);
    fn position(&self) -> Duration;
    /// True once the end was reached after `play`.
    fn finished(&self) -> bool;
}
pub enum OpenError { Unsupported, NoOutput, Io }          // closed; the UI words each once
pub struct AudioFactory(pub fn() -> Box<dyn AudioOutput>); // a gpui Global
```

The real implementation lives in `desktop/src/audio.rs` and is the **only** file that touches a platform audio API. Candidates, with what is known (details to be filled in by R3-R5; nothing below is measured yet):

| | Backend | New code and deps | Formats | Known risks |
|---|---|---|---|---|
| A | `AVAudioPlayer` through `objc2-avf-audio` (+ `objc2-foundation`: `NSURL`, `NSError`) | the objc2 family is already in the tree (`objc2 =0.6.5`, `objc2-app-kit`, `objc2-foundation`); one small unsafe-free wrapper (the crate's calls are `unsafe fn`s) | whatever Core Audio reads: AAC/M4A, MP3, WAV, FLAC, CAF; [inferred] **not** Ogg or WebM Opus | may not open every sender's voice (R3-R4) |
| B | `rodio` + `symphonia` (+ `cpal` under it) | several crates; licence of symphonia [INFERENCE: MPL-2.0] | Vorbis, MP3, AAC, ISO-MP4, Ogg container; Opus only if the pinned release has a real decoder (the repository's dependency graph says nothing, R5 checks) | codec coverage, build weight |
| C | Ogg + libopus + `cpal`/`rodio` | native libopus (a build step) or a pure-Rust adapter | Opus | native toolchain need (the project avoids full Xcode), more code |

**Rule.** Use the smallest candidate that opens **every** voice sample R3 observed: A if R4 shows `AVAudioPlayer` opens all of them; else B if R5 shows the pinned symphonia decodes the rest (including Opus) and builds on the Command Line Tools alone; else C under the same two conditions; if none qualifies the main session decides before any voice code is written (the video part is unaffected). Backend choice changes only `audio.rs`, `Cargo.toml`, `THIRD_PARTY.md` (task 4.2); everything else depends on the trait, so a second backend added later for a second format is one more `impl` behind a dispatcher in `audio.rs`, not a redesign. A format no backend can read shows the failure «Не удалось воспроизвести» and keeps Download.

Tests never use the real backend: `Desktop::new` keeps its signature and `Player` reads the factory from a gpui **global** (`cx.try_global::<AudioFactory>()`); `main.rs` registers the real one, the test harness registers a scripted fake in `tests.rs::harness_with`. No global means "no sound output": the failure `NoOutput`. (Rejected: a constructor parameter, which changes every test's `Desktop::new` call; a `cfg(test)` fake inside `audio.rs`, which hides the real path from review. Same reasoning as `add-reveal-downloaded-file` D7.)

### D6. The player: state machine, one at a time, when it stops

```rust
// desktop/src/player.rs, an Entity created by Desktop::new
pub struct VoiceKey { pub chat: ChatId, pub timestamp: MessageTs }   // per message, not per file id
pub enum VoicePhase { Idle, Loading, Playing, Paused, Failed(PlayFailure) }
pub enum PlayFailure { Download(DownloadFailure), Unsupported, NoOutput }
pub struct VoiceView { pub phase: VoicePhase, pub position: Duration, pub total: Option<Duration> }
impl Player {
    pub fn voice(&self, key: &VoiceKey) -> VoiceView;                    // Idle for any other message
    pub fn toggle_voice(&mut self, key: VoiceKey, original: &MediaRef, name: Option<&str>, cx);
    pub fn stop(&mut self, cx);                                          // back to Idle, position 0
    pub fn open_video(&mut self, original: &MediaRef, name: &str, cx);   // D8
}
pub enum PlayerEvent { OpenExternal(PathBuf) }                           // video hand-off, handled in main.rs
```

- **Click on a bubble's control** (`toggle_voice`): `Idle`/`Failed` -> start: if the store holds `Saved(path)` for `original.playback()` and the file exists, open it; else ask the store `download(original.playback(), name)` (online only; offline with no copy gives `Failed(Download(Offline))`, shown as unavailable) and go `Loading`; `Loading` -> cancel the transfer, `Idle`; `Playing` -> pause; `Paused` -> resume. When the store reports `Saved` for the pending key, open the file and play. A failure of the transfer or `open` is `Failed(..)`; the next click is the retry.
- **One at a time:** `toggle_voice` for another message stops the current one (it returns to `Idle`, position 0) before starting. A single `Box<dyn AudioOutput>` is created lazily and reused.
- **Position:** while `Playing`, a 200 ms timer (`cx.background_executor().timer`, as `store.rs::typing_started` does) reads `position()` and `finished()`; at the end the phase becomes `Idle` (position 0; pressing play plays from the start). The timer runs only while `Playing` and ends with it (a generation counter drops stale ticks). Each tick `cx.notify()`s the player; the views observe it, so only the rows that read it change, and a row's height never depends on the state (D7).
- **It stops** (`stop`) when: the open chat changes, the message of the playing key is removed from the timeline, the session ends (`StoreEvent::Session`), the window closes (the entity drops), or the connection state does not matter (the file is local, playback continues offline). Rationale: no global mini player in this change (non-goal), and a sound that goes on with no visible control is a defect; so audio never outlives the bubble that controls it.
- `total`: the wire duration until the file is open, then the file's own length from `AudioOutput::open`; the displayed total does not change after it is first shown unless the two differ by more than one second (then the file's length wins), so the label does not flicker.
- `Player` holds the store `Entity` to call `download`/`cancel_download`/`download_state` and subscribes to its events (`StoreEvent::Download(media)` for the pending key); it never talks to the core directly. Unit tests drive it with the fake output and the store harness.

### D7. The voice bubble

From the mockup (`docs/mockups/1-main.svg`): a 40 px circle (`BADGE` `0x3d84c6`, close to the mockup's `#5288c1`; no new theme colour), a glyph inside (text, not an icon asset: «▶» idle, «⏸» playing, «…» loading, «▶» after a failure), then bars, then the time. Fixed geometry so a row never changes size (the file card's rule, `message-history`): the content is 300 px wide (`CONTENT_W` cap) with the circle 40, a 10 px gap, **28 bars** 3 px wide at a 6 px pitch (168 px, as in the mockup), and the time label right of them; below it the existing one-line **action row** (18 px) for the original's Download state, reused as is (`action_row` with the voice's name, status text, «Показать в Finder», «Скачать»), so voice gets the saved/progress/cancel/retry behaviour of `attachment-download` with no new code.

- **Bars:** the waveform resampled (nearest-bar max) to 28 values, scaled to 4-26 px; without a waveform, 28 flat 8 px bars. Bars before the playhead (position / total of 28) use `ACCENT`, the rest `MUTED`. While `Loading` no bar is lit.
- **Time label:** `m:ss`; idle and paused-at-zero show the total, playing and paused show the elapsed time; a missing total shows nothing until known. New pure helper `format::duration_label(ms)`.
- **Failure text:** the action row is the only status line and it belongs to Download, so a playback failure takes the **place of the time label** (`MUTED`, one short text): «Нет сети», «Файл недоступен», «Формат не поддерживается», «Нет вывода звука», «Ошибка»; the next click on the circle retries. One text per `PlayFailure`/`DownloadFailure` case, chosen once in a pure function (testable without a window).
- Selectors (`debug_selector`): `msg-{ts}-voice` (bubble), `-voice-toggle` (circle), `-voice-bars`, `-voice-time`, plus the action row's existing `-download|-cancel|-retry|-saved|-status|-reveal`. Element id `voice-toggle-{ts}`.
- Deleted messages and messages with `Delivery::Pending` never get a player (nothing is pending for a voice: sending media is a non-goal).

### D8. Video: hand-off to the system player

Chosen: fetch a playback copy (D3) with the card showing «Загрузка для просмотра: 3,2 из 80 МБ» and a cancel control, then emit `PlayerEvent::OpenExternal(path)`; `main.rs` calls `cx.open_with_system(&path)`. A later click on the same card opens the existing copy at once.

Why: `gpui` has no video playback (R7), a native inline player would be a new unsafe AVFoundation integration with per-frame display plumbing and no way to test it headless, and the system player already does everything a video needs (seek, full screen, volume, codecs). The decision follows `add-reveal-downloaded-file` D1/D7 in spirit: one thin platform call, in `main.rs`, because the test platform's `open_with_system` is `unimplemented!()`; the store and `Player` tests assert the emitted event.

Rejected:
- **Inline `Surface` + `AVPlayer`** (`AVPlayerItemVideoOutput` -> `CVPixelBuffer` -> `paint_surface`): feasible (R7) but large, unsafe, untestable without a display, plays audio outside gpui anyway. Left as a possible later change.
- **Quick Look** (`qlmanage -p`, or `QLPreviewPanel`): a subprocess or a new objc integration for a preview that has no seek control worth the name.
- **A `wry` WebView** (already a dependency, sign-in only): a second window and WebKit codec limits for no gain over the system player.
- **Opening the Downloads copy:** forbidden by `attachment-download` ("the client SHALL NOT open … a saved file"); Download and Play stay independent, so playing after a download fetches a second copy. Accepted.
- **Streaming while downloading, or `video_streams`:** needs Range or a manifest the system player cannot authenticate (D4).

**Safety.** `open` launches whatever application the extension maps to, including applications for some extensions (`.app`, `.command`, `.terminal`, scripts). So the hand-off happens **only** for a copy whose extension is in a fixed whitelist: `mp4, m4v, mov, 3gp, mpg, mpeg, avi, mkv, webm` (research R3/R6 may shorten it, never extend it without a review). The whitelist is applied twice: on the **name** in the adapter (a body that fails it stays a file card, D2) and on the **actual path** in `Player::open_video` immediately before every `OpenExternal` (the guarantee, next paragraph; a failing path emits no event and shows the failure «Формат не поддерживается»). The playback copy has mode 0600 in a 0700 directory, it carries no quarantine attribute (the client sets none on downloads either); the whitelist is what keeps this safe, not quarantine.

**The check is on the path that is opened, not on the name.** The store's `Saved(path)` is keyed by `Id2`, so a copy may have been written for another message holding the same file under another name (a voice message named `x.command`, a forward renamed to `.mp4`). `Player::open_video` therefore takes the extension from the **actual `path`** (the last `.ext` of its file name, case-insensitive, no extension fails) immediately before emitting `OpenExternal`, on both routes: the fresh `Downloaded` and the reuse of an existing `Saved`. A reused copy that fails is **not opened and is treated as absent**: the player calls `store.download` (it starts a new transfer even when the state is `Saved`) with the video's own name, which the adapter already limited to the whitelist, so the new copy is `media_file_name` plus a whitelisted extension (a different file name from the failed one) and replaces the `Saved` state; the old file stays until the next wipe at start or sign-out. A fresh copy whose path still fails the check emits no event and the card shows «Формат не поддерживается». Offline with only a failing copy: unavailable, no event. The adapter's check of the name (D2) decides the card's look only; it is not the safety guarantee.

The card (`timeline.rs`): the same 300 px layout as `file_card`, with a **44 px circle holding «▶»** in place of the extension badge (the circle is the Play control; while loading it shows «✕» and cancels), the name, the line «0:42 · 12 МБ» (`format::duration_size_line`: duration and `human_size`), and the existing action row for Download. Selectors: `msg-{ts}-video`, `-video-play`, `-video-status`. A failure shows its reason in the size line; clicking the circle retries. Never starts by itself.

### D9. Download stays independent of Play

Voice and video bubbles show the Download action row with the original's key `media.original()` (D7, D8), exactly as a file card: same `store.download`, same Downloads destination, name rules, progress, cancel, retry, reveal. Voice's download name is `FileInfo.Name`, else «Голосовое сообщение» plus the extension research R3 identifies (none if unknown; the sanitiser's fallback name is «Файл»). Play's transfer is a different key (D3) so both can run at once. The attachment-download spec changes only where the new behaviour touches it (two modified requirements).

### D10. What is deliberately not built, and why

- **Speed, seeking, "listened" mark:** each is additive; none is asked for ("play/pause, progress, duration, speed optional"); speed would also need backend support verified per candidate (`enableRate` on `AVAudioPlayer`, `Sink::set_speed` on rodio). The trait is not widened for them, so there is no dead code. Follow-ups.
- **Mini player / playback across chats:** the stop rule of D6 is the price of not building it.
- **Transcript:** the `Text` field is not shown (non-goal); research R1 only records whether it is ever non-empty.
- **Cache cap or eviction for playback copies:** removal at start and sign-out only (D3); a cap needs a policy the user did not ask for.

### D11. Privacy and logging

A playback transfer is a download: the session cookie goes only to its host (redirects drop it), and the existing rules of `attachment-download` ("Authenticated and private transfer") cover it. No log line at `debug` or above names a path or a file name, includes a query string, or says which message is played; `player.rs` logs nothing at all, the engine's existing `download saved` line (byte count) is the only line. The hand-off passes the path to `open` and logs nothing. Playing marks nothing as read or listened and sends nothing: the playback code path has no write request.

### D12. Order and merge units (`DEPENDENCIES.md` rules 1-3)

- Research tasks 0.x first; code tasks marked blocked wait for the research they name (their stated default applies if the research cannot run, see `research.md`).
- **PB-V** = the voice unit (tasks 1.1, 1.2, 2.1, 2.2, 3.1, 4.1, 4.2, 5.1, 5.2, 5.3, 5.5, 5.6): `MessageBody::Voice` constructed by the adapter, matched by the timeline, played by `Player` through the real output registered in `main.rs`; none of these can reach `main` alone without a `dead_code` warning or a stub (rule 3). Branch `integ/add-media-playback`.
- **PB-W** = the video unit (1.3, 2.3, 3.2, 5.4, 5.7, 5.8), after PB-V merged (it extends `model.rs`, `yandex_backend.rs`, `timeline.rs`, `player.rs`, `main.rs`, `tests.rs` that PB-V holds).
- Task 0.2 (media targets and the probe) is standalone: library variants and a probe that uses them.

## Risks / Trade-offs

- **Unobserved audio format and duration unit** (the dominant risk): the backend rule of D5 and the stated defaults make the code tasks conditional on research instead of guessing.
- **A format no backend reads** for some senders: the bubble shows a failure and Download still works; a second backend is one `impl`.
- **The VM cannot confirm audible output.** Tests assert state transitions through the fake and the real backend's position advance in the VM (R4, task 6.2); "it sounds right" is a hand check on a real Mac by the user, reported as a limit, not a gate.
- **5 Hz re-render of the observing views while a voice plays.** Rows have fixed size, so no layout work follows; task 5.3 checks that a tick does not rebuild history and does not move the scroll position.
- **Disk use of playback copies** until the next start (a large video is a large file). Accepted; progress and cancel are visible, and Download to the user's own folder is separate.
- **`open_with_system` outcome is invisible** (gpui discards it): a missing handler silently does nothing. Accepted, as with reveal.
- **The unofficial protocol** (`file_shortterm`, `PreviewHint`) can change; the research results are the record of what was observed.
