# Research: playing voice messages and video attachments

Written 2026-11-28 before any code. Predecessor: `openspec/changes/archive/2026-11-25-add-media-download/research.md` (section 3, item 3 "Voice/video playback: separate scope after the URLs, the audio/video formats and the local GPUI/macOS support are confirmed", section 4 "play only on an explicit click", section 5 "audio/video need format, streaming and URL-authorisation checks"). This file is the single evidence document of this change: the **research tasks come first** (what is unknown, how it is measured, what each answer decides), then the facts already established, then a results log the tasks fill in.

Rules for every task here (`openspec/changes/DEPENDENCIES.md`): cargo, `bt-probe`, the app and every sample inspection run **only in the Tart VM `bt-vm`**, each agent in its own `~/bt/src-<name>` (procedure: `docs/vm.md` section 3), removed afterwards. `[live]` tasks use the signed-in session in the VM's `~/bt/.runtime` and the real account; they share the one live slot (rule 5: one live task at a time; `BT_WRITE_ONLY_CHAT` stays set). **All `[live]` tasks below are reads only: they use the app account's own session, send nothing, change nothing on the server, and need no user.** Samples (audio, video) are downloaded into a VM temp directory, inspected there and deleted when the task ends; nothing from a sample is committed (no bytes, no file names, no chat ids, no text). Only counts, container/codec names, sizes, key names and value types go into the results log.

## Research tasks

### R1 [live] What the account holds: voice and video messages, and the shape of their wire fields

- **Question.** Does the app account's history contain `Voice` and video `MiscFile` messages at all, from whom (own, other people, which client), how many, and what exactly do the two wire objects carry beyond what `wire.rs` decodes today?
- **Method.** In `bt-vm`, `bt-probe chats`, then `BT_LOG=yandex_proto::xiva=trace bt-probe history <chat>` (and `--before` for older pages) over the chats, the trace saved to a VM temp file and counted with `grep -c` per chat (the chat is reported by ordinal, never by id or name). From the raw `Voice` and `MiscFile` objects record **key names and value types only**: `Voice.{FileInfo{Id2,Name,Size,Source},Duration,Text,WasRecognized,Waveform,DisableRecognition}` (which are present, the type and length of `Waveform`, whether `Text` is ever non-empty), `MiscFile.{FileInfo,PreviewHint.VideoPreview{Width,Height,DurationMs,BlurHash}}` (is `PreviewHint` present for every video, are there videos without it, what `Source` values occur, what extensions the names have). Delete the trace afterwards.
- **Decides.** Whether R2-R6 can run at all (no sample means they are blocked); the `Voice.Duration` unit question together with R3; how a video is recognised (design D2); whether the waveform is decodable (D7); whether a voice transcript is trivially present (it is a non-goal either way, the observation only goes into the follow-ups).
- **If nothing is found.** The task records "none". The fallback is the same one PIN 0.2 used: the web client of `admin@sgnl.pro`, the *other party* of an existing private chat with the app account, sends a video (a file upload of a short clip) and, if the browser can be given a fake microphone, a voice message, while the app account only reads. That writes nothing from the app account, but it is the main session's call whether the precedent applies; if it does not, R2-R6 stay blocked and the design keeps its stated defaults.
- **Needs the user.** No.
- **Files.** `research.md` only.

### R2 [code, then live] The bytes: URLs, redirects, headers, sizes

- **Question.** Which request returns playable bytes for a voice message and for a video: `file_shortterm/{Id2}` (the notes' `voicePlayUrl`), `file_shortterm/{Id2}?attach=true` (today's `MediaTarget::Original`, used by Download), or `video_streams/{Id2}` (the notes' `videoStreamsUrl`)? For each: status, whether a redirect to a storage host happens (host name only), `Content-Type`, `Content-Length`, `Accept-Ranges`, whether `Content-Disposition` is set, and whether the bytes of the plain and the `attach=true` variants are identical (sha256 equal). Does a `Source: 1` (DISK) file behave differently? Does `video_streams` answer with a playlist (HLS/DASH) or a plain file?
- **Method.** Code part (needs no account): `MediaTarget::Playback` (`file_shortterm/{id}`, no query) and `MediaTarget::VideoStream` (`video_streams/{id}`) in `yandex-proto/src/media.rs` with URL unit tests, and `bt-probe download <chat> <ts> [--url original|plain|stream]` accepting a voice or video message (`select_file` today takes only File, Image, Gallery) and printing, for the request, only: status, landed host, `Content-Type`, `Content-Length`, `Accept-Ranges`, bytes received, sha256 of the body. Live part: run it on the samples R1 found, saving into a VM temp directory.
- **Decides.** The URL of the playback copy (design D4, task 1.2), whether `VideoStream` is dropped again (it is when `plain` or `original` returns the whole file), whether a Range-based streaming start is even possible (it is not planned: D3).
- **Needs the user.** No. Reads of file bytes with the app session; the only write is to the VM temp directory.
- **Files.** `yandex-proto/src/media.rs`, `yandex-proto/src/bin/bt-probe.rs`, `research.md`.

### R3 [live, on the R2 samples] Audio and video formats, the duration unit, the waveform

- **Question.** Container and codec of the voice samples (Ogg/Opus, WebM/Opus, M4A/AAC, others?), sample rate, channels, bitrate, and whether the format depends on who sent it (web client, phone); the real length of each sample against `Voice.Duration` (so seconds or milliseconds); how `Waveform` is encoded (base64 of one byte per bar? its length against the duration); the container and codec of the video samples, and the file name extensions they carry.
- **Method.** In the VM: `file --mime-type`, `afinfo <file>` (Core Audio's own view: it prints the format or fails, which is the direct answer to "can Core Audio read this"), `mdls -name kMDItemDurationSeconds -name kMDItemCodecs <file>`; for the waveform decode the base64 and count bytes against the bar count the web client draws (compare with the duration only; no audio content is looked at).
- **Decides.** The unit conversion of `Voice.Duration` (design D2), the audio backend (D5, the decision rule there), the waveform decoder or its absence (D7), the video extension whitelist (D8).
- **Needs the user.** No.
- **Files.** `research.md`.

### R4 [VM, no account] Does the VM have a sound output, and does `AVAudioPlayer` open the samples?

- **Question.** Has `bt-vm` an output device at all (`system_profiler SPAudioDataType`)? Through Core Audio's own player (`AVAudioPlayer`, driven from `osascript -l JavaScript` with `ObjC.import('AVFoundation')`, no cargo needed): does `initWithContentsOfURL:error:` succeed for each R3 sample, what `duration` does it report, does `play` return true and does `currentTime` advance over two seconds, does `playing` turn false at the end?
- **Method.** As stated, in a short JXA script run with `tart exec bt-vm osascript -l JavaScript`; on any sample the VM cannot open, the result is the error text. If the VM has no output device, the result also records what `play` does then (the position cannot be heard to advance but may still advance). A real, audible check is impossible in the VM and is recorded as a limit, not as a failure.
- **Decides.** The first branch of the backend rule (design D5): `AVAudioPlayer` through `objc2-avf-audio` if every sample opens, and how tests and the VM hand check (task 6.2) can observe playback.
- **Needs the user.** No. Local files only, no account.
- **Files.** `research.md`.

### R5 [VM, no account] Candidate audio crates: licence, dependency weight, buildability

- **Question.** For each candidate backend of design D5: (A) `objc2-avf-audio` (+ `objc2-foundation` features) at versions compatible with the pinned `objc2 =0.6.5`, `objc2-app-kit =0.3.2`; (B) `rodio` + `symphonia` (features for exactly the observed formats; is there an Opus decoder in the released symphonia at the version used, or only a stub); (C) an explicit Ogg + libopus decoder (`ogg` + `opus` or a symphonia libopus adapter) feeding `cpal` or `rodio`. For each: licence of every new crate (`cargo tree -e normal --format "{p} {l}"`), number of new crates in `Cargo.lock`, whether it builds with the Command Line Tools only (no full Xcode, no cmake; the repository already builds shaders at run time for this reason, `crates/desktop/Cargo.toml`), clean build time, and whether the project's `--locked` flow still works with the new lock entries.
- **Method.** A scratch crate in `~/bt/src-<name>/scratch` in the VM, outside the repository tree, depending on the candidate only (the VM's outbound network works again, `DEPENDENCIES.md`). Nothing in the repository changes. Required only when R4 says branch A is not enough; branch A's buildability with the pinned objc2 set is checked either way, because A is the default.
- **Decides.** The backend (design D5), the `THIRD_PARTY.md` lines (an MPL-2.0 crate, if any, needs a note: [INFERENCE] symphonia is MPL-2.0, to be confirmed here), task 4.2's dependency list.
- **Needs the user.** No.
- **Files.** `research.md`.

### R6 [VM, no account] Does handing a video to the system player work from the app's path?

- **Question.** With a downloaded video sample (or, without one, a two-second clip made in the VM with `screencapture -V 2 <file>.mov`): does `open <path>` (what gpui's `App::open_with_system` runs on macOS, `gpui-0.2.2/src/platform/mac/platform.rs:856`) start a player process (`pgrep -x "QuickTime Player"` or the default handler) and open the file? Is the file's extension what picks the handler (copy a sample to a name with no extension and to a hash-like name with `.mp4`; record the result for both)? What does `open` do with an extension outside the whitelist (checked only for a harmless made-up extension, never for an executable type)?
- **Method.** `tart exec bt-vm`, `open`, `pgrep`, one screenshot through `.runtime/vm/shot.sh` if needed. No app run.
- **Decides.** That the hand-off needs a recognised extension on the playback copy (design D3, D8), and the whitelist.
- **Needs the user.** No.
- **Files.** `research.md`.

### R7 [done 2026-11-28, host, read-only] What `gpui` 0.2.2 offers for audio and video

Source read in the local registry copy `gpui-0.2.2` (the pinned version, `crates/desktop/Cargo.toml`); nothing was run.

- **Audio.** No audio API at all: a search of `gpui-0.2.2/src` for `audio`, `AVPlayer`, `AVFoundation`, `play_sound` finds nothing. Sound output must come from outside gpui.
- **Video.** `gpui` can paint a macOS `CVPixelBuffer` (`Window::paint_surface`, `elements/surface.rs`, `SurfaceSource::Surface(CVPixelBuffer)`), so an inline player is possible only by decoding with AVFoundation (`AVPlayer` + `AVPlayerItemVideoOutput`) and feeding frames per display tick; no decoding or playback of its own exists. Two other things in the repository could carry video: `wry` (a WebView, used only for the sign-in page) and `gpui`'s `img` (still images only).
- **Hand-off.** `App::open_with_system(&Path)` on macOS spawns `open <path>` on the background executor and ignores the result; `App::reveal_path` selects in Finder (already used by «Показать в Finder»). In the test platform `open_with_system` and `reveal_path` are `unimplemented!()`, so the call has to live outside the view and store code the tests run (the same reason as `add-reveal-downloaded-file` D7).
- **Decides.** Design D8: video goes to the system player; the inline `Surface` + `AVPlayer` route is recorded as a rejected alternative.

## Facts already established (sources)

Marked **[repo]** (read in this repository on 2026-11-28), **[notes]** (the reverse-engineering notes `conarti/yandex-messenger-mcp`, `docs/protocol-research.md`, an unofficial and not a stable contract), **[inferred]**.

1. **[repo]** `wire.rs`: `WireBody::Voice { file: WireFile, duration: Option<u64> }` is decoded from `Plain.Voice` (`FileInfo` plus `Duration`); `Waveform`, `Text`, `WasRecognized`, `DisableRecognition` are not read. `WireFile { id (Id2, `<bucket>/<uuid>`), name, size }`. A video is not told apart from any file: `Plain.MiscFile` becomes `WireBody::File`; `PreviewHint` is not read.
2. **[repo]** `yandex_backend.rs:1248`: a voice message becomes `MessageBody::Unsupported("Голосовое сообщение")`, shown as the placeholder "[Голосовое сообщение]" (`MessageBody::preview_text`). A video-with-hint is a plain file card with Download.
3. **[repo]** `media.rs`: `MediaTarget::{Avatar, Thumbnail, Original}`; `Original` is `files_base/file_shortterm/{id}?attach=true`, streamed by `MediaClient::download` with no size cap, cookie only for the matching host, 401 = session expired, 404 = not found, truncated body = `Network`. `MediaClient::fetch` is buffered with a 25 MiB cap.
4. **[repo]** chat-core: `Command::Download { media (kind File), name }` -> `Engine::start_download` -> `run_download` (own task, slot semaphore `DOWNLOAD_SLOTS`, `create_partial`/`commit_partial` in `config.downloads_dir`, `ClientEvent::{DownloadProgress, Downloaded, DownloadFailed, DownloadCancelled}`); `sign_out` removes `media_dir(runtime, account)` recursively; `fetch_media` refuses `MediaKind::File`. The desktop store keeps `downloads: HashMap<MediaRef, DownloadState>` in memory only, cleared at `SignedOut`.
5. **[repo]** `store.rs::want_media` fetches only avatars and thumbnails on its own, never a `MediaKind::File` ref.
6. **[repo]** The fixtures hold one **synthetic** voice message (`research-history-page.json`: `Voice { FileInfo { Id2, Size: 4000 }, Duration: 12 }`); no captured voice or video message exists in the repository.
7. **[repo]** Mockups: `docs/mockups/1-main.svg` (and `3`, `6`, `7`, `8`) draw a voice bubble: a 40 px circle in `#5288c1` with a «▶» glyph, a row of 3 px bars at a 6 px pitch in `#6ab3f3` as the waveform, the time «0:14» in the footer line. No video bubble is drawn.
8. **[notes]** `Plain.MiscFile = { FileInfo, PreviewHint: { VideoPreview: { Width, Height, DurationMs, BlurHash } } }`; `Plain.Voice = { FileInfo, Duration, Text, WasRecognized, Waveform, DisableRecognition }`; `FileInfo = { Id2, Name, Size, Source (0 MDS, 1 DISK) }` (section 11.1, 12.4).
9. **[notes]** URL templates from the live app config (section 12.2): `voicePlayUrl` = `https://files.messenger.yandex.ru/file_shortterm/{fileId}`, `fileDownloadUrl` = `.../file_shortterm/{fileId}?attach=true`, `videoStreamsUrl` = `.../video_streams/{fileId}`; access needs the same session; `file_shortterm` answers `302` to a storage host and the redirect must be followed (section 17.9); `FileInfo.Id2` contains `/` and must be encoded per segment (the repository's `safe_path` already does this). The audio format and the unit of `Voice.Duration` are **not** stated anywhere in the notes.
10. **[inferred]** A voice message recorded in a browser is usually WebM/Opus (Chrome's `MediaRecorder`) or Ogg/Opus (Firefox) and one from a phone is usually AAC in M4A or Opus; `AVAudioPlayer` reads AAC and not Ogg/WebM Opus. So a single `AVAudioPlayer`-only backend may not cover every sender. This is exactly what R3 and R4 settle; nothing in the design assumes either outcome without the rule of D5.
11. **[repo]** `THIRD_PARTY.md`/`openspec/config.yaml`: the project is MIT; it relicenses to GPL-3.0 only when the first Telegram Desktop graphic is reused. This change draws its icons as text glyphs («▶», «⏸», «✕», «…») and uses no Telegram graphics, so no relicence is triggered. A new dependency is recorded in `THIRD_PARTY.md` only when its licence needs a note.

## Results log

Filled in by the tasks above, newest last. Format: task id, date, who, then the answers (booleans, names of formats, counts, key names; no content, no ids).

- R1: not run.
- R2: not run.
- R3: not run.
- R4: not run.
- R5: not run.
- R6: not run.
- R7: done 2026-11-28, see above.

## What each unknown decides (summary)

| Unknown | Answered by | Decides | If it cannot be answered |
|---|---|---|---|
| Does the account hold voice/video samples | R1 | whether R2-R6 can run | web client as other party (main session's call), else blocked |
| `Voice.Duration` unit | R3 (with R1) | the conversion in the adapter (D2) | the bubble shows the decoded duration once loaded and none before; the adapter treats the raw value as seconds [INFERENCE] until R3 |
| Audio format(s) of voice messages | R3, R4 | backend branch (D5) | branch A is built first; a sample it cannot open turns on the next branch |
| Waveform encoding | R1, R3 | decoder or flat bars (D7) | flat bars; no decoder written |
| Playable URL, redirect, range | R2 | `MediaTarget::Playback` URL (D4) | `file_shortterm/{id}?attach=true` as for Download (known to work) |
| Video recognition and extensions | R1, R3 | whitelist (D8) | `PreviewHint` present and extension in the default whitelist |
| Hand-off works, needs an extension | R6 | D3 naming, D8 | the whitelist plus `open` is still the plan; a failure reopens D8 |
| Sound output in the VM | R4 | how task 6.2 is observed | position-advance check instead of listening |
