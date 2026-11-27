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
WireBody::Voice { file: WireFile, duration: Option<u64> /* ms, raw */, waveform: Option<Vec<u8>> /* levels 0..=15 */ }
WireBody::Video(WireVideo)      // a MiscFile that carries PreviewHint.VideoPreview; every other MiscFile stays WireBody::File
pub struct WireVideo { pub file: WireFile, pub width: u32, pub height: u32, pub duration_ms: u64 }

// chat-core/src/model.rs
MediaKind::Playback                      // a playback copy of an original; MediaRef.id is the Id2
impl MediaRef { pub fn playback(&self) -> MediaRef }   // {id, kind: Playback}, like original()
MessageBody::Voice { media: MediaRef /*kind File*/, duration_ms: Option<u64>, size: u64,
                     name: Option<String>, waveform: Option<Vec<u8>> }
MessageBody::Video { media: MediaRef /*kind File*/, name: String, size: u64,
                     width: u32, height: u32, duration_ms: Option<u64> /* None without the hint */ }
```

- `Voice.Duration` is **milliseconds** (R3, 17 of 17). The wire stores it raw and the adapter copies it to `duration_ms` unchanged, no conversion. It is the web client's wall-clock stopwatch, so it can exceed the audio's real length by 89 to 280 ms (10 of 17 voices; `SpeechKit` ones are within 15 ms): the label never needs correcting, and D6's rule (the file's own length wins only when it differs by more than one second) keeps it from flickering. A missing duration is `None`; the bubble then shows no time until the audio is loaded.
- **Waveform** (R3): the string is base64 of packed 4-bit levels, **two per byte, high nibble first**; 136 characters = 100 or 101 bytes = **200 or 202 values**, each 0 to 15 (a dB-scaled peak level, 0 = silence). `wire.rs` base64-decodes, takes `v >> 4` then `v & 15` of every byte and keeps the values as `Vec<u8>` of 0..=15 (any length is accepted). An empty result or a string that is not valid base64 is `None`, never an error. Bars are drawn from the values by D7; without a waveform they are flat.
- **A video is a `MiscFile` whose name's extension is in the video whitelist of D8** (`mp4`, `mov`, case-insensitive); **`PreviewHint` is optional**. R1: the hint is present on only 11 of 94 video-extension files (12 %), so requiring it would leave most videos as file cards; the extension, not the hint, is what the hand-off depends on (R6) and what keeps it safe. When `PreviewHint.VideoPreview` is present, `wire.rs` yields `WireBody::Video` and the adapter takes `width`, `height` and `duration_ms` from it (`BlurHash` ignored; a missing key is 0, and a zero duration is `None`). Without it (wire `File`) the adapter builds the same `MessageBody::Video` with zeros and `duration_ms: None`, and the card shows the size only. A file whose extension is outside the whitelist stays `MessageBody::File`, the file card as today, hint or not (the five hinted `.webm` videos, and `.mkv`: QuickTime opens a `.webm` with an error alert and `.mkv` has no handler, R6). A non-video file named `x.mp4` gets a card whose hand-off ends in QuickTime's error alert (R6, Matroska named `.mp4`): harmless, accepted.
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

### D4. URL of the playback copy

`MediaTarget::Playback { file_id }` = `files_base/file_shortterm/{id}` with **no query** (the notes' `voicePlayUrl`; the same host, session cookie and redirect handling as `Original`, `safe_path` for the id). Used for voice and video alike. R2 measured it on 17 voices and 7 video files: **200** after a redirect to a storage host, the whole file, `Content-Length` equal to `FileInfo.Size`, `Accept-Ranges: bytes`, and the body **byte-identical to `?attach=true`** (sha256 equal for 17 of 17 voices and 3 of 3 `.mp4`), so the plain URL needs no `attach=true` fallback and `Source` has one case (`1` on every `MiscFile`, absent on a voice). `video_streams/{id}` is **not** used: for a video it answers a JSON manifest of six HLS playlists on a third host (`streaming.disk.yandex.net`) that the system player cannot open with our cookie, for a voice it is `400`; **`MediaTarget::VideoStream` and the probe's `stream` value are deleted in task 1.2**. No Range request and no streaming start (D3). `adapter::download_target` maps `Playback` -> `MediaTarget::Playback`, `File` -> `Original` as now.

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

The real implementation lives in `desktop/src/audio.rs` and is the **only** file that touches a platform audio API. **Chosen: A-engine**, by the rule below and the measurements of R3-R5 on the account's 17 voice messages (all Ogg Opus, mono, 48 kHz decoded; R3):

| | Backend | New code and deps | Result on the 17 voices | Verdict |
|---|---|---|---|---|
| **A-engine** | `objc2-avf-audio =0.3.2`: `AVAudioFile` + `AVAudioEngine` + `AVAudioPlayerNode` | **1 new crate** (Zlib OR Apache-2.0 OR MIT, the licence family of the objc2 crates already shipped); `objc2 =0.6.5`, `objc2-foundation`, `block2 =0.6.2` already in the tree; Command Line Tools only; `--locked` clean once the lock is generated; the crate's calls are `unsafe fn`s | **plays 17 of 17** to the end; pause, resume, stop, replay work; the end comes from a completion handler | **chosen** |
| A | the same crate, `AVAudioPlayer` alone | same | opens 17 of 17 but **plays 10** (below) | rejected |
| B | `rodio` + `symphonia` (+ `cpal`) | 21 lock entries (14 built), six MPL-2.0 `symphonia*` crates | **0 of 17**: no Opus decoder in symphonia 0.5.5 or 0.6.1 | rejected |
| C1, C2 | libopus (`opus`, or `symphonia-adapter-libopus`) | `opusic-sys` builds libopus with `cmake`, absent on a Command-Line-Tools-only machine | C1 17 of 17; C2 one sample (566 s) decodes 1.77 s short | rejected: not CLT-only |
| C3, C5 | `ogg` + pure-Rust Opus (`opus-decoder`, `opus-rs`) | 2 crates | 17 of 17; C3 runs 15 to 25 times real time on the 566 s sample | not needed; C5 after C4, C3 last |
| **C4** | `ogg =0.9.2` + `ropus =0.12.18` + `objc2-audio-toolbox =0.3.2` (`AudioQueue`) | 6 crates, all permissive, CLT only; our own decode loop, PCM queue and `unsafe extern "C-unwind"` buffer callback | 17 of 17, frame counts and SNR equal to libopus; 566 s in 0.5 s (release) | **reserve** |

**Why `AVAudioPlayer` alone is out (R4, reproduced in Rust by R5):** `initWithContentsOfURL:error:` succeeds for all 17 and reports the right `duration`, but `prepareToPlay` and `play` return **false for 7 of them**: the web-recorded voices (encoder tag `opusconvertfromwebm`; 6 own, 1 from another person; 2.3 to 12.7 s; 4517 to 23928 bytes), with no `NSError`, `isPlaying` false and `currentTime` 0. It is deterministic and independent of the file's directory, extension, `initWithData`, a file-type hint, process and order; `afconvert` decodes the same files, so Core Audio's reader and Opus decoder are fine and only `AVAudioPlayer`'s queue refuses the stream (the unified log shows the converter property `abtd` rejected). Opening is not playing, so the first branch of the original rule did not hold. The engine path reads the same files through `AVAudioFile` and plays them.

**Rule (as applied).** The smallest candidate that **plays** every observed voice: A fails, A-engine qualifies, so B and C are recorded only so that nobody re-runs the comparison. Reserve order if the engine path proves unusable in a way not seen (the one unobserved case is a missing output device): C4, then C5, then C3. Backend choice changes only `audio.rs`, `Cargo.toml` and `Cargo.lock` (task 4.2; `THIRD_PARTY.md` needs no line, the licences are permissive); everything else depends on the trait, so a second backend later is one more `impl` behind a dispatcher in `audio.rs`, not a redesign. A format no backend can read shows the failure «Формат не поддерживается» and keeps Download.

**Facts for task 4.2** (R5, observed in scratch runs with the calls 4.2 will make):
- Cargo: `objc2-avf-audio =0.3.2`, `default-features = false`, features `std`, `block2`, `AVAudioFile`, `AVAudioEngine`, `AVAudioPlayerNode`, `AVAudioNode`, `AVAudioFormat`, `AVAudioTime`, `AVAudioMixerNode`, `AVAudioTypes` (`AVAudioPlayer` is not needed). The lock gains exactly one entry; generate it once without `--locked`. The dependency and `audio.rs` land in one commit (cargo's `unused_dependencies` warns until the crate is used, rule 3).
- Errors: a path that does not exist and an empty file give `NSOSStatusErrorDomain` 2003334207, 7 bytes of garbage 1685348671, a `.webm`/`.mkv` 1954115647 (`AVAudioPlayer` and `AVAudioFile` alike). So `Io` comes from `std::fs` **before** the call and `Unsupported` from the Core Audio error. A truncated Ogg Opus file opens with no error and **length 0**: a zero length is `Unsupported`. A failing `startAndReturnError` is `NoOutput` (not observed: the VM has an output device).
- Open: `AnyThread::alloc()` for `initForReading_error`; the length is `file.length() / file.processingFormat().sampleRate()` (equal to R3's real length).
- Graph: `AVAudioEngine::new()`, `attachNode`, `connect_to_format(&node, &engine.mainMixerNode(), Some(&format))`, `startAndReturnError`; `scheduleFile_atTime_completionCallbackType_completionHandler(&file, None, DataPlayedBack, block)` with a `block2::RcBlock` that **runs on another thread** (an `Arc` with an atomic is enough). **Always schedule before `play`** (a `play` without a schedule runs the clock ahead of the audio).
- Position: `node.lastRenderTime()` -> `playerTimeForNodeTime` -> `sampleTime / sampleRate`; it is `None` before the first render and **while paused**, so keep the last position read before `pause`; clamp it to the length (the node time keeps counting past the end); after a natural end the node clock does **not** restart, so a run's position is the node time minus the node time read right after its `play` (or `stop()` first, which restarts it at 0).
- End: `finished()` comes from the completion handler, never from `isPlaying` (it stays true after a natural end). `stop()` fires the handler **at once**, so the handler must be ignored after our own `stop`: a generation counter captured by the block, bumped by `stop` and by every new `scheduleFile`.
- Controls: `pause` then `play` resumes in place (the handler does not fire on a pause); after a natural end or a stop, `scheduleFile` + `play` on the same node replays; `engine.stop()` then `startAndReturnError()` works and the node stays attached.

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

- **Bars:** the waveform (levels 0 to 15, D2) resampled (nearest-bar max) to 28 values, scaled to 4-26 px (`v / 15`); without a waveform, 28 flat 8 px bars. Bars before the playhead (position / total of 28) use `ACCENT`, the rest `MUTED`. While `Loading` no bar is lit.
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

**Safety.** `open` launches whatever application the extension maps to, including applications for some extensions (`.app`, `.command`, `.terminal`, scripts). So the hand-off happens **only** for a copy whose extension is in a fixed whitelist: **`mp4`, `mov`** (case-insensitive). It is what R6 measured on this VM: `open` of `.mp4`, `.MP4`, `.MOV`, and a 64-hex hash name with `.mp4` start QuickTime Player (exit code 0) with a document window; the extension alone picks the handler (the content is not sniffed), a copy with **no extension opens in TextEdit as text** and one with a made-up extension fails with `kLSApplicationNotFoundErr` (exit code 1, nothing started), which is why the copy name keeps a real extension (D3). Dropped from the first draft of the list: `m4v`, `3gp`, `mpg`, `mpeg`, `avi` (never observed in the account, hand-off not tested), `webm` (QuickTime starts and shows an error alert, no document) and `mkv` (no handler); a later change may extend the list only after measuring the type and a review. The whitelist is applied twice: on the **name** in the adapter (a body that fails it stays a file card, D2) and on the **actual path** in `Player::open_video` immediately before every `OpenExternal` (the guarantee, next paragraph; a failing path emits no event and shows the failure «Формат не поддерживается»). The playback copy has mode 0600 in a 0700 directory, it carries no quarantine attribute (the client sets none on downloads either); the whitelist is what keeps this safe, not quarantine.

**The check is on the path that is opened, not on the name.** The store's `Saved(path)` is keyed by `Id2`, so a copy may have been written for another message holding the same file under another name (a voice message named `x.command`, a forward renamed to `.mp4`). `Player::open_video` therefore takes the extension from the **actual `path`** (the last `.ext` of its file name, case-insensitive, no extension fails) immediately before emitting `OpenExternal`, on both routes: the fresh `Downloaded` and the reuse of an existing `Saved`. A reused copy that fails is **not opened and is treated as absent**: the player calls `store.download` (it starts a new transfer even when the state is `Saved`) with the video's own name, which the adapter already limited to the whitelist, so the new copy is `media_file_name` plus a whitelisted extension (a different file name from the failed one) and replaces the `Saved` state; the old file stays until the next wipe at start or sign-out. A fresh copy whose path still fails the check emits no event and the card shows «Формат не поддерживается». Offline with only a failing copy: unavailable, no event. The adapter's check of the name (D2) decides the card's look only; it is not the safety guarantee.

The card (`timeline.rs`): the same 300 px layout as `file_card`, with a **44 px circle holding «▶»** in place of the extension badge (the circle is the Play control; while loading it shows «✕» and cancels), the name, the line «0:42 · 12,3 МБ» (duration and `human_size`; **the size alone when there is no duration**, D2), and the existing action row for Download. Selectors: `msg-{ts}-video`, `-video-play`, `-video-status`. A failure shows its reason in the size line; clicking the circle retries. Never starts by itself.

### D9. Download stays independent of Play

Voice and video bubbles show the Download action row with the original's key `media.original()` (D7, D8), exactly as a file card: same `store.download`, same Downloads destination, name rules, progress, cancel, retry, reveal. Voice's download name is `FileInfo.Name` (R1: present and `.ogg` on all 17), else «Голосовое сообщение» with no extension (the code's `VOICE_NAME`; it is only a fallback for a name the server never omitted in the account). Play's transfer is a different key (D3) so both can run at once. The attachment-download spec changes only where the new behaviour touches it (two modified requirements).

### D10. What is deliberately not built, and why

- **Speed, seeking, "listened" mark:** each is additive; none is asked for ("play/pause, progress, duration, speed optional"); speed would also need a time-pitch or varispeed unit in the engine graph and seeking a segment schedule (R4: seeking in Ogg Opus through `AVAudioPlayer` is unreliable). The trait is not widened for them, so there is no dead code. Follow-ups.
- **Mini player / playback across chats:** the stop rule of D6 is the price of not building it.
- **Transcript:** the `Text` field is not shown (non-goal); R1 observed it non-empty (with `WasRecognized` true) on all 17 voices, so a later change could show it for free.
- **Cache cap or eviction for playback copies:** removal at start and sign-out only (D3); a cap needs a policy the user did not ask for.

### D11. Privacy and logging

A playback transfer is a download: the session cookie goes only to its host (redirects drop it), and the existing rules of `attachment-download` ("Authenticated and private transfer") cover it. No log line at `debug` or above names a path or a file name, includes a query string, or says which message is played; `player.rs` logs nothing at all, the engine's existing `download saved` line (byte count) is the only line. The hand-off passes the path to `open` and logs nothing. Playing marks nothing as read or listened and sends nothing: the playback code path has no write request.

### D12. Order and merge units (`DEPENDENCIES.md` rules 1-3)

- Research tasks 0.x are closed (0.8); the answers are written into D2, D4, D5 and D8 and no code task waits for research any more.
- **PB-V** = the voice unit (tasks 1.1, 1.2, 2.1, 2.2, 3.1, 4.1, 4.2, 5.1, 5.2, 5.3, 5.5, 5.6): `MessageBody::Voice` constructed by the adapter, matched by the timeline, played by `Player` through the real output registered in `main.rs`; none of these can reach `main` alone without a `dead_code` warning or a stub (rule 3). Branch `integ/add-media-playback`.
- **PB-W** = the video unit (1.3, 2.3, 3.2, 5.4, 5.7, 5.8), after PB-V merged (it extends `model.rs`, `yandex_backend.rs`, `timeline.rs`, `player.rs`, `main.rs`, `tests.rs` that PB-V holds).
- Task 0.2 (media targets and the probe) is standalone: library variants and a probe that uses them.

## Risks / Trade-offs

- **Voice format and duration unit** were the dominant risk and are measured (R3-R5): Ogg Opus on 17 of 17, `Duration` in ms, `AVAudioPlayer` alone plays 10 of 17, hence A-engine (D5). Residual: one account only; no AAC/M4A phone voice was seen (the engine path played an AAC MP4 sample), and `engine.start` with no output device is unobserved (mapped to `NoOutput`, checked by hand in 6.2).
- **A format no backend reads** for some sender: the bubble shows a failure and Download still works; a second backend is one `impl` (C4 is the measured reserve).
- **The VM cannot confirm audible output.** Tests assert state transitions through the fake and the real backend's position advance in the VM (R4, task 6.2); "it sounds right" is a hand check on a real Mac by the user, reported as a limit, not a gate.
- **5 Hz re-render of the observing views while a voice plays.** Rows have fixed size, so no layout work follows; task 5.3 checks that a tick does not rebuild history and does not move the scroll position.
- **Disk use of playback copies** until the next start (a large video is a large file). Accepted; progress and cancel are visible, and Download to the user's own folder is separate.
- **`open_with_system` outcome is invisible** (gpui discards it): a missing handler silently does nothing. Accepted, as with reveal.
- **The unofficial protocol** (`file_shortterm`, `PreviewHint`) can change; the research results are the record of what was observed.
