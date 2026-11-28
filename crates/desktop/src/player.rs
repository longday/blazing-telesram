//! Voice playback: an audio output behind `AudioOutput` and the state machine that drives it.
//!
//! `Player` owns no UI. Views read `voice(key)` and call `toggle_voice`; the playback copy of the
//! file comes from the store (`ChatStore::download` with the `Playback` kind), the sound from the
//! `AudioOutput` the registered `AudioFactory` makes. No factory registered means no sound output.
//! A video is never decoded here: `open_video` fetches its playback copy and emits
//! `PlayerEvent::OpenExternal` for a copy whose extension is whitelisted; `main.rs` hands it to the
//! system (design D8).
//! Nothing here logs: no path, file name or message is ever written to a log line.

use crate::store::{ChatStore, DownloadState, SessionState, StoreEvent};
use crate::yandex_backend::is_video_name;
use bt_chat_core::model::{ChatId, DownloadFailure, MediaRef, MessageTs};
use gpui::{App, Context, Entity, EventEmitter, Global, Subscription};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How often the position and the end of the playing file are read.
pub const POSITION_TICK: Duration = Duration::from_millis(200);

/// One audio output, driven by `Player` from the UI thread. The only implementation that touches a
/// platform audio API is `audio.rs`; tests use the scripted fake of this module.
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

/// Why a file could not be opened or started; closed on purpose, the UI words each case once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// No decoder reads this file.
    Unsupported,
    /// There is no sound output device.
    NoOutput,
    /// The file could not be read.
    Io,
}

/// Makes the output the `Player` uses. A gpui global: the app registers the real one, the test
/// harness a scripted fake; no global means "no sound output".
pub struct AudioFactory(pub fn() -> Box<dyn AudioOutput>);

impl Global for AudioFactory {}

/// A voice message, per message and not per file id: the same file forwarded twice has two keys.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VoiceKey {
    pub chat: ChatId,
    pub timestamp: MessageTs,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VoicePhase {
    #[default]
    Idle,
    Loading,
    Playing,
    Paused,
    Failed(PlayFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayFailure {
    Download(DownloadFailure),
    Unsupported,
    NoOutput,
}

impl From<OpenError> for PlayFailure {
    fn from(error: OpenError) -> Self {
        match error {
            OpenError::Unsupported => Self::Unsupported,
            OpenError::NoOutput => Self::NoOutput,
            // A copy that was saved and cannot be read is a failure of the disk.
            OpenError::Io => Self::Download(DownloadFailure::Disk),
        }
    }
}

/// The one short text that takes the place of the time label after a failure (design D7).
pub fn failure_label(failure: PlayFailure) -> &'static str {
    match failure {
        PlayFailure::Download(DownloadFailure::Offline | DownloadFailure::Network) => "Нет сети",
        PlayFailure::Download(DownloadFailure::NotFound | DownloadFailure::Disk) => {
            "Файл недоступен"
        }
        PlayFailure::Download(DownloadFailure::Destination | DownloadFailure::Other) => "Ошибка",
        PlayFailure::Unsupported => "Формат не поддерживается",
        PlayFailure::NoOutput => "Нет вывода звука",
    }
}

/// What a bubble shows. `total` is the length of the file as last opened for this message, `None`
/// until it was opened (the bubble then shows the duration the message carries).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceView {
    pub phase: VoicePhase,
    pub position: Duration,
    pub total: Option<Duration>,
}

/// The message that is not idle. Every other message is `Idle` with position 0.
struct Current {
    key: VoiceKey,
    /// The store key of its playback copy.
    playback: MediaRef,
    /// Never `Idle`: that is "no `Current`".
    phase: VoicePhase,
    position: Duration,
    total: Option<Duration>,
    /// A start that waits for the confirmation of a cancel in flight: the name to download under.
    restart: Option<Option<String>>,
}

pub struct Player {
    store: Entity<ChatStore>,
    /// Made on the first start and reused.
    output: Option<Box<dyn AudioOutput>>,
    current: Option<Current>,
    /// Bumped whenever the phase leaves or enters `Playing`; a timer tick of an older one is stale.
    generation: u64,
    /// Playback copies whose `CancelDownload` was sent and not yet answered.
    cancelling: Vec<MediaRef>,
    /// The length of the file the last time each message's file was opened.
    measured: HashMap<VoiceKey, Duration>,
    /// Videos whose playback copy is being fetched or whose open failed, per playback copy. A
    /// video that was handed to the system, cancelled or never asked is absent (idle).
    videos: HashMap<MediaRef, VideoOpen>,
    _store_events: Subscription,
}

/// What the player asks the app to do outside the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlayerEvent {
    /// Open this playback copy of a video in the system's player. Only emitted for a path whose
    /// extension passed the whitelist of design D8 just before.
    OpenExternal(PathBuf),
}

impl EventEmitter<PlayerEvent> for Player {}

/// A video that is not idle.
struct VideoOpen {
    /// The video's own name (the adapter limited it to the whitelist), the name of its copy.
    name: String,
    /// `Loading` or `Failed`.
    phase: VoicePhase,
    /// A start that waits for the confirmation of a cancel in flight.
    restart: bool,
}

/// The last extension of the file name of `path` is a whitelisted video extension (design D8).
/// The path is what gets opened, so it is the path that is checked, never the name of the message.
fn is_video_path(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(is_video_name)
}

impl Player {
    pub fn new(store: Entity<ChatStore>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&store, |this, _, event: &StoreEvent, cx| {
            this.on_store_event(event, cx)
        });
        Self {
            store,
            output: None,
            current: None,
            generation: 0,
            cancelling: Vec::new(),
            measured: HashMap::new(),
            videos: HashMap::new(),
            _store_events: subscription,
        }
    }

    /// A message that is not the active one is `Idle` at 0, with the length measured the last time
    /// its file was opened (so the bubble keeps it after the end, a stop or another message).
    pub fn voice(&self, key: &VoiceKey) -> VoiceView {
        let measured = self.measured.get(key).copied();
        match &self.current {
            Some(c) if &c.key == key => VoiceView {
                phase: c.phase,
                position: c.position,
                total: c.total.or(measured),
            },
            _ => VoiceView {
                total: measured,
                ..VoiceView::default()
            },
        }
    }

    /// The message that is loading, playing, paused or failed; `None` when every message is idle.
    pub fn active(&self) -> Option<&VoiceKey> {
        self.current.as_ref().map(|c| &c.key)
    }

    /// The click on a bubble's control. `Idle` and `Failed` start (a failure's retry), `Loading`
    /// cancels the transfer, `Playing` pauses, `Paused` resumes. Starting stops any other message
    /// first, except that a message still waiting for the same file takes over its transfer.
    /// `original` is the message's file (kind `File`); its playback copy is fetched under
    /// `name`.
    pub fn toggle_voice(
        &mut self,
        key: VoiceKey,
        original: &MediaRef,
        name: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let phase = (self.current.as_ref())
            .filter(|c| c.key == key)
            .map(|c| c.phase);
        match phase {
            Some(VoicePhase::Loading) => return self.stop(cx),
            Some(VoicePhase::Playing) => return self.pause(cx),
            Some(VoicePhase::Paused) => return self.resume(cx),
            Some(VoicePhase::Idle | VoicePhase::Failed(_)) | None => {}
        }
        // Another message of the same file (a forward) that is waiting for the copy takes over the
        // transfer: cancelling it would clear the new message when `DownloadCancelled` arrives.
        let playback = original.playback();
        if let Some(current) = (self.current.as_mut())
            .filter(|c| c.phase == VoicePhase::Loading && c.playback == playback)
        {
            current.key = key;
            cx.notify();
            return;
        }
        self.stop(cx);
        self.start(key, playback, name, cx);
    }

    /// Back to `Idle`, position 0, for whatever is loading, playing or paused (or failed).
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let Some(current) = self.current.take() else {
            return;
        };
        match current.phase {
            VoicePhase::Loading => self.cancel_transfer(current.playback, cx),
            VoicePhase::Playing | VoicePhase::Paused => {
                if let Some(output) = self.output.as_mut() {
                    output.stop();
                }
            }
            VoicePhase::Idle | VoicePhase::Failed(_) => {}
        }
        cx.notify();
    }

    fn start(
        &mut self,
        key: VoiceKey,
        playback: MediaRef,
        name: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.current = Some(Current {
            key,
            playback: playback.clone(),
            phase: VoicePhase::Loading,
            position: Duration::ZERO,
            total: None,
            restart: None,
        });
        cx.notify();
        if !self.ensure_output(cx) {
            return self.fail(PlayFailure::NoOutput, cx);
        }
        // A copy of this run is reused, also offline; one that vanished from the disk is fetched again.
        let saved = match self.store.read(cx).download_state(&playback) {
            Some(DownloadState::Saved(path)) if path.is_file() => Some(path.clone()),
            _ => None,
        };
        match saved {
            Some(path) => self.play_file(&path, cx),
            None => {
                let name = name.map(str::to_owned);
                let cancel_out = self.cancelling.contains(&playback)
                    && matches!(
                        self.store.read(cx).download_state(&playback),
                        Some(DownloadState::Running { .. })
                    );
                if cancel_out {
                    // The store holds the cancelled transfer as running until the core confirms:
                    // `download` would send nothing and the confirmation would clear this start.
                    if let Some(current) = self.current.as_mut() {
                        current.restart = Some(name);
                    }
                } else {
                    self.request_download(name, cx);
                }
            }
        }
    }

    /// Asks the store for the playback copy of the loading message under `name`.
    fn request_download(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        let Some(playback) = self.current.as_ref().map(|c| c.playback.clone()) else {
            return;
        };
        self.store
            .update(cx, |s, cx| s.download(playback, name, cx));
        self.settle_download(cx);
    }

    fn ensure_output(&mut self, cx: &App) -> bool {
        if self.output.is_none() {
            self.output = cx.try_global::<AudioFactory>().map(|f| (f.0)());
        }
        self.output.is_some()
    }

    /// Acts on what the store says about the playback copy of the loading message.
    fn settle_download(&mut self, cx: &mut Context<Self>) {
        let Some(current) = self.current.as_ref() else {
            return;
        };
        match self.store.read(cx).download_state(&current.playback) {
            Some(DownloadState::Saved(path)) => {
                let path = path.clone();
                self.play_file(&path, cx);
            }
            Some(DownloadState::Failed(reason)) => {
                let failure = PlayFailure::Download(*reason);
                self.fail(failure, cx);
            }
            // The transfer was cancelled (by us, or the store forgot it); a start that was waiting
            // for the confirmation asks now.
            None => match self.current.as_mut().and_then(|c| c.restart.take()) {
                Some(name) => self.request_download(name, cx),
                None => {
                    self.generation += 1;
                    self.current = None;
                    cx.notify();
                }
            },
            Some(DownloadState::Running { .. }) => {}
        }
    }

    fn play_file(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(output) = self.output.as_mut() else {
            return self.fail(PlayFailure::NoOutput, cx);
        };
        let total = match output.open(path) {
            Ok(total) => total,
            Err(error) => return self.fail(error.into(), cx),
        };
        if let Err(error) = output.play() {
            output.stop();
            return self.fail(error.into(), cx);
        }
        if let Some(current) = self.current.as_mut() {
            current.phase = VoicePhase::Playing;
            current.position = Duration::ZERO;
            current.total = Some(total);
            self.measured.insert(current.key.clone(), total);
        }
        self.start_timer(cx);
        cx.notify();
    }

    fn pause(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let (Some(current), Some(output)) = (self.current.as_mut(), self.output.as_mut()) else {
            return;
        };
        output.pause();
        current.position = output.position();
        current.phase = VoicePhase::Paused;
        cx.notify();
    }

    fn resume(&mut self, cx: &mut Context<Self>) {
        let Some(output) = self.output.as_mut() else {
            return self.fail(PlayFailure::NoOutput, cx);
        };
        if let Err(error) = output.play() {
            output.stop();
            return self.fail(error.into(), cx);
        }
        if let Some(current) = self.current.as_mut() {
            current.phase = VoicePhase::Playing;
        }
        self.start_timer(cx);
        cx.notify();
    }

    fn fail(&mut self, failure: PlayFailure, cx: &mut Context<Self>) {
        self.generation += 1;
        if let Some(current) = self.current.as_mut() {
            current.phase = VoicePhase::Failed(failure);
            current.position = Duration::ZERO;
            current.total = None;
        }
        cx.notify();
    }

    /// Starts the timer of a new playing stretch; the previous one, if any, ends at its next tick.
    fn start_timer(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POSITION_TICK).await;
                match this.update(cx, |player, cx| player.tick(generation, cx)) {
                    Ok(true) => {}
                    _ => break,
                }
            }
        })
        .detach();
    }

    /// Reads the output once. Returns whether the timer goes on: not for a stale tick, not once
    /// the phase is no longer `Playing`, not at the end of the file.
    fn tick(&mut self, generation: u64, cx: &mut Context<Self>) -> bool {
        if generation != self.generation {
            return false;
        }
        let (Some(current), Some(output)) = (self.current.as_mut(), self.output.as_mut()) else {
            return false;
        };
        if current.phase != VoicePhase::Playing {
            return false;
        }
        if output.finished() {
            output.stop();
            self.generation += 1;
            self.current = None;
            cx.notify();
            return false;
        }
        let position = output.position();
        if position != current.position {
            current.position = position;
            cx.notify();
        }
        true
    }

    /// Stops and drops what was learned of the messages shown so far (the end of a session or a
    /// switch of the account).
    fn forget(&mut self, cx: &mut Context<Self>) {
        self.stop(cx);
        let videos = std::mem::take(&mut self.videos);
        if !videos.is_empty() {
            cx.notify();
        }
        for (playback, video) in videos {
            if video.phase == VoicePhase::Loading {
                self.cancel_transfer(playback, cx);
            }
        }
        self.cancelling.clear();
        self.measured.clear();
    }

    /// Asks the core to stop the running fetch of `playback`, once; the store clears the state
    /// when the core confirms.
    fn cancel_transfer(&mut self, playback: MediaRef, cx: &App) {
        let store = self.store.read(cx);
        if matches!(
            store.download_state(&playback),
            Some(DownloadState::Running { .. })
        ) {
            store.cancel_download(&playback);
            if !self.cancelling.contains(&playback) {
                self.cancelling.push(playback);
            }
        }
    }

    fn on_store_event(&mut self, event: &StoreEvent, cx: &mut Context<Self>) {
        match event {
            StoreEvent::Download(media) => {
                let running = matches!(
                    self.store.read(cx).download_state(media),
                    Some(DownloadState::Running { .. })
                );
                if !running {
                    self.cancelling.retain(|m| m != media);
                }
                let loading = self
                    .current
                    .as_ref()
                    .is_some_and(|c| c.phase == VoicePhase::Loading && &c.playback == media);
                if loading {
                    self.settle_download(cx);
                }
                if self
                    .videos
                    .get(media)
                    .is_some_and(|v| v.phase == VoicePhase::Loading)
                {
                    self.settle_video(media, cx);
                }
            }
            StoreEvent::Session if self.store.read(cx).session() != SessionState::Active => {
                self.forget(cx)
            }
            // Another account: its voice messages are gone, so is what the player knew of them.
            StoreEvent::Account => self.forget(cx),
            _ => {}
        }
    }

    /// Where the playback copy of the video `original` stands: `Loading` while it is fetched,
    /// `Failed(..)` after a failure (the next `open_video` retries), `Idle` otherwise.
    pub fn video(&self, original: &MediaRef) -> VoicePhase {
        (self.videos.get(&original.playback())).map_or(VoicePhase::Idle, |v| v.phase)
    }

    /// The click on a video card's control. While the copy is being fetched it cancels the fetch;
    /// otherwise an existing copy (also offline) is opened at once and a missing one is fetched
    /// under `name`, the video's own name, then opened. A copy is opened by emitting
    /// `PlayerEvent::OpenExternal`, and only if the extension of its actual path is whitelisted:
    /// a reused copy that fails the check (the same file saved earlier for a message with another
    /// name) is treated as absent, a fresh one that fails is `Failed(Unsupported)`.
    pub fn open_video(&mut self, original: &MediaRef, name: &str, cx: &mut Context<Self>) {
        let playback = original.playback();
        let loading = (self.videos.get(&playback)).is_some_and(|v| v.phase == VoicePhase::Loading);
        if loading {
            self.videos.remove(&playback);
            self.cancel_transfer(playback, cx);
            cx.notify();
            return;
        }
        let reused = match self.store.read(cx).download_state(&playback) {
            Some(DownloadState::Saved(path)) if path.is_file() => Some(path.clone()),
            _ => None,
        };
        if reused.is_some_and(|path| self.emit_open(path, cx)) {
            self.videos.remove(&playback);
            cx.notify();
            return;
        }
        // As for voice: the store still holds a cancelled transfer as running until the core
        // confirms, and `download` would send nothing then.
        let cancel_out = self.cancelling.contains(&playback)
            && matches!(
                self.store.read(cx).download_state(&playback),
                Some(DownloadState::Running { .. })
            );
        let video = VideoOpen {
            name: name.to_owned(),
            phase: VoicePhase::Loading,
            restart: cancel_out,
        };
        self.videos.insert(playback.clone(), video);
        cx.notify();
        if !cancel_out {
            self.request_video(&playback, cx);
        }
    }

    /// The one place `OpenExternal` is emitted: only for a path that passes the whitelist now.
    fn emit_open(&mut self, path: PathBuf, cx: &mut Context<Self>) -> bool {
        let allowed = is_video_path(&path);
        if allowed {
            cx.emit(PlayerEvent::OpenExternal(path));
        }
        allowed
    }

    /// Asks the store for the playback copy of a loading video under its own name.
    fn request_video(&mut self, playback: &MediaRef, cx: &mut Context<Self>) {
        let Some(name) = self.videos.get(playback).map(|v| v.name.clone()) else {
            return;
        };
        self.store
            .update(cx, |s, cx| s.download(playback.clone(), Some(name), cx));
        self.settle_video(playback, cx);
    }

    /// Acts on what the store says about the playback copy of a loading video.
    fn settle_video(&mut self, playback: &MediaRef, cx: &mut Context<Self>) {
        if !self.videos.contains_key(playback) {
            return;
        }
        match self.store.read(cx).download_state(playback).cloned() {
            Some(DownloadState::Saved(path)) => {
                let opened = self.emit_open(path, cx);
                self.finish_video(playback, (!opened).then_some(PlayFailure::Unsupported), cx);
            }
            Some(DownloadState::Failed(reason)) => {
                self.finish_video(playback, Some(PlayFailure::Download(reason)), cx);
            }
            // Cancelled (by us, or the store forgot it): a start that waited for the confirmation
            // asks now, otherwise the card is idle again.
            None => {
                let restart =
                    (self.videos.get_mut(playback)).is_some_and(|v| std::mem::take(&mut v.restart));
                if restart {
                    self.request_video(playback, cx);
                } else {
                    self.finish_video(playback, None, cx);
                }
            }
            Some(DownloadState::Running { .. }) => {}
        }
    }

    /// Ends the loading of a video: idle (`None`) or failed.
    fn finish_video(
        &mut self,
        playback: &MediaRef,
        failure: Option<PlayFailure>,
        cx: &mut Context<Self>,
    ) {
        match failure {
            Some(failure) => {
                if let Some(video) = self.videos.get_mut(playback) {
                    video.phase = VoicePhase::Failed(failure);
                }
            }
            None => {
                self.videos.remove(playback);
            }
        }
        cx.notify();
    }
}

impl Drop for Player {
    /// The window closed: the sound ends with it.
    fn drop(&mut self) {
        if let Some(output) = self.output.as_mut() {
            output.stop();
        }
    }
}

/// A scripted `AudioOutput` for tests: the factory is a plain `fn`, so the script lives in a
/// thread-local (a gpui test runs on one thread) and `Fake` is the handle to it.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    /// What the player asked of the output, in order.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Call {
        Open(PathBuf),
        Play,
        Pause,
        Stop,
    }

    struct Script {
        open: Result<Duration, OpenError>,
        play: Result<(), OpenError>,
        position: Duration,
        finished: bool,
        calls: Vec<Call>,
        position_reads: usize,
        created: usize,
    }

    thread_local! {
        static SCRIPT: RefCell<Script> = RefCell::new(Script::new());
    }

    impl Script {
        fn new() -> Self {
            Self {
                open: Ok(Duration::from_secs(10)),
                play: Ok(()),
                position: Duration::ZERO,
                finished: false,
                calls: Vec::new(),
                position_reads: 0,
                created: 0,
            }
        }
    }

    fn script<R>(f: impl FnOnce(&mut Script) -> R) -> R {
        SCRIPT.with(|s| f(&mut s.borrow_mut()))
    }

    /// The handle to the script of this thread's fake output.
    pub struct Fake;

    /// Resets the script: opens succeed with a length of 10 s, nothing was called.
    pub fn reset() -> Fake {
        script(|s| *s = Script::new());
        Fake
    }

    /// Registers the fake as the app's audio factory.
    pub fn install(cx: &mut App) {
        cx.set_global(AudioFactory(|| {
            script(|s| s.created += 1);
            Box::new(Output)
        }));
    }

    impl Fake {
        /// The result of every following `open`.
        pub fn open_returns(&self, result: Result<Duration, OpenError>) {
            script(|s| s.open = result);
        }

        /// The result of every following `play`.
        pub fn play_returns(&self, result: Result<(), OpenError>) {
            script(|s| s.play = result);
        }

        pub fn set_position(&self, position: Duration) {
            script(|s| s.position = position);
        }

        /// The end of the file is reached.
        pub fn finish(&self) {
            script(|s| s.finished = true);
        }

        /// The calls since the last take.
        pub fn take_calls(&self) -> Vec<Call> {
            script(|s| std::mem::take(&mut s.calls))
        }

        pub fn position_reads(&self) -> usize {
            script(|s| s.position_reads)
        }

        /// How many outputs the factory made.
        pub fn created(&self) -> usize {
            script(|s| s.created)
        }
    }

    struct Output;

    impl AudioOutput for Output {
        fn open(&mut self, path: &Path) -> Result<Duration, OpenError> {
            script(|s| {
                s.calls.push(Call::Open(path.to_path_buf()));
                s.position = Duration::ZERO;
                s.finished = false;
                s.open
            })
        }

        fn play(&mut self) -> Result<(), OpenError> {
            script(|s| {
                s.calls.push(Call::Play);
                s.play
            })
        }

        fn pause(&mut self) {
            script(|s| s.calls.push(Call::Pause));
        }

        fn stop(&mut self) {
            script(|s| {
                s.calls.push(Call::Stop);
                s.position = Duration::ZERO;
                s.finished = false;
            });
        }

        fn position(&self) -> Duration {
            script(|s| {
                s.position_reads += 1;
                s.position
            })
        }

        fn finished(&self) -> bool {
            script(|s| s.finished)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{self, Call, Fake};
    use super::*;
    use async_channel::{Receiver, Sender};
    use bt_chat_core::model::{ClientEvent, Command, ConnectionState, MediaKind};
    use gpui::{AppContext, TestAppContext};
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

    const SECS: fn(u64) -> Duration = Duration::from_secs;

    fn chat() -> ChatId {
        ChatId("c".into())
    }

    fn key(ts: MessageTs) -> VoiceKey {
        VoiceKey {
            chat: chat(),
            timestamp: ts,
        }
    }

    fn original(file: &str) -> MediaRef {
        MediaRef {
            id: file.into(),
            kind: MediaKind::File,
        }
    }

    fn playback(file: &str) -> MediaRef {
        original(file).playback()
    }

    struct Rig {
        store: Entity<ChatStore>,
        player: Entity<Player>,
        commands: Receiver<Command>,
        fake: Fake,
        dir: tempfile::TempDir,
        /// The `PlayerEvent`s since the last take.
        events: Rc<RefCell<Vec<PlayerEvent>>>,
        _client: Sender<ClientEvent>,
    }

    impl Rig {
        /// Online, with the scripted output registered.
        fn new(cx: &mut TestAppContext) -> Self {
            Self::build(cx, true)
        }

        /// Online, with no audio factory registered.
        fn without_output(cx: &mut TestAppContext) -> Self {
            Self::build(cx, false)
        }

        fn build(cx: &mut TestAppContext, output: bool) -> Self {
            let fake = fake::reset();
            if output {
                cx.update(fake::install);
            }
            let (client, event_rx) = async_channel::unbounded();
            let (command_tx, commands) = async_channel::unbounded();
            let store = cx.new(|cx| ChatStore::new(event_rx, command_tx, cx));
            let player = cx.new(|cx| Player::new(store.clone(), cx));
            let events = Rc::new(RefCell::new(Vec::new()));
            let sink = events.clone();
            cx.update(|cx| {
                cx.subscribe(&player, move |_, event: &PlayerEvent, _| {
                    sink.borrow_mut().push(event.clone())
                })
                .detach()
            });
            let rig = Self {
                store,
                player,
                commands,
                fake,
                dir: tempfile::tempdir().unwrap(),
                events,
                _client: client,
            };
            rig.apply(cx, ClientEvent::Connection(ConnectionState::Online));
            rig
        }

        fn apply(&self, cx: &mut TestAppContext, event: ClientEvent) {
            self.store.update(cx, |s, cx| s.apply(event, cx));
            cx.run_until_parked();
        }

        fn commands(&self) -> Vec<Command> {
            std::iter::from_fn(|| self.commands.try_recv().ok()).collect()
        }

        /// The click on message `ts`, whose file is `file`.
        fn toggle(&self, cx: &mut TestAppContext, ts: MessageTs, file: &str) {
            let original = original(file);
            self.player.update(cx, |p, cx| {
                p.toggle_voice(key(ts), &original, Some("v.ogg"), cx)
            });
            cx.run_until_parked();
        }

        fn view(&self, cx: &mut TestAppContext, ts: MessageTs) -> VoiceView {
            self.player.read_with(cx, |p, _| p.voice(&key(ts)))
        }

        fn phase(&self, cx: &mut TestAppContext, ts: MessageTs) -> VoicePhase {
            self.view(cx, ts).phase
        }

        /// The store learns that the playback copy of `file` is on disk; returns its path.
        fn saved(&self, cx: &mut TestAppContext, file: &str) -> PathBuf {
            self.saved_as(cx, file, &format!("{file}.ogg"))
        }

        /// The same, with the copy saved under the file name `name`.
        fn saved_as(&self, cx: &mut TestAppContext, file: &str, name: &str) -> PathBuf {
            let path = self.dir.path().join(name);
            std::fs::write(&path, b"data").unwrap();
            self.apply(
                cx,
                ClientEvent::Downloaded {
                    media: playback(file),
                    path: path.clone(),
                },
            );
            path
        }

        /// The click on the card of the video `file`, whose own name is `name`.
        fn open(&self, cx: &mut TestAppContext, file: &str, name: &str) {
            let original = original(file);
            self.player
                .update(cx, |p, cx| p.open_video(&original, name, cx));
            cx.run_until_parked();
        }

        fn video(&self, cx: &mut TestAppContext, file: &str) -> VoicePhase {
            self.player.read_with(cx, |p, _| p.video(&original(file)))
        }

        fn events(&self) -> Vec<PlayerEvent> {
            std::mem::take(&mut *self.events.borrow_mut())
        }

        /// Message `ts` plays its (already saved) file `file`; the calls and commands so far are dropped.
        fn playing(&self, cx: &mut TestAppContext, ts: MessageTs, file: &str) -> PathBuf {
            let path = self.saved(cx, file);
            self.toggle(cx, ts, file);
            assert_eq!(self.phase(cx, ts), VoicePhase::Playing);
            self.fake.take_calls();
            self.commands();
            path
        }
    }

    /// Idle at 0 after the file was opened: the length the fake measured (10 s) is kept.
    fn idle_after_open() -> VoiceView {
        VoiceView {
            total: Some(SECS(10)),
            ..VoiceView::default()
        }
    }

    /// Lets `millis` of the test clock pass and runs what became due.
    fn wait(cx: &mut TestAppContext, millis: u64) {
        cx.executor().advance_clock(Duration::from_millis(millis));
        cx.run_until_parked();
    }

    #[gpui::test]
    fn first_play_asks_for_the_playback_copy_and_plays_on_saved(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.toggle(cx, 1, "f1");
        assert_eq!(rig.phase(cx, 1), VoicePhase::Loading);
        assert_eq!(
            rig.commands(),
            [Command::Download {
                media: playback("f1"),
                name: Some("v.ogg".into()),
            }]
        );
        assert_eq!(
            rig.fake.take_calls(),
            [],
            "no sound before the copy is on disk"
        );

        // The copy of another file does not start this message.
        rig.saved(cx, "other");
        assert_eq!(rig.phase(cx, 1), VoicePhase::Loading);
        assert_eq!(rig.fake.take_calls(), []);

        let path = rig.saved(cx, "f1");
        assert_eq!(
            rig.view(cx, 1),
            VoiceView {
                phase: VoicePhase::Playing,
                position: Duration::ZERO,
                total: Some(SECS(10)),
            }
        );
        assert_eq!(rig.fake.take_calls(), [Call::Open(path), Call::Play]);
        assert_eq!(
            rig.view(cx, 2),
            VoiceView::default(),
            "any other message is idle"
        );
    }

    #[gpui::test]
    fn an_existing_copy_plays_at_once_and_offline(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        let path = rig.saved(cx, "f1");
        rig.apply(cx, ClientEvent::Connection(ConnectionState::Offline));
        rig.toggle(cx, 1, "f1");
        assert_eq!(rig.phase(cx, 1), VoicePhase::Playing);
        assert_eq!(rig.fake.take_calls(), [Call::Open(path), Call::Play]);
        assert_eq!(rig.commands(), [], "nothing is fetched");
    }

    #[gpui::test]
    fn no_copy_offline_fails_without_a_command(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.apply(cx, ClientEvent::Connection(ConnectionState::Offline));
        rig.toggle(cx, 1, "f1");
        assert_eq!(
            rig.phase(cx, 1),
            VoicePhase::Failed(PlayFailure::Download(DownloadFailure::Offline))
        );
        assert_eq!(rig.commands(), []);
        assert_eq!(rig.fake.take_calls(), []);
    }

    #[gpui::test]
    fn a_copy_that_vanished_from_the_disk_is_fetched_again(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        let path = rig.saved(cx, "f1");
        std::fs::remove_file(&path).unwrap();
        rig.toggle(cx, 1, "f1");
        assert_eq!(rig.phase(cx, 1), VoicePhase::Loading);
        assert_eq!(rig.commands().len(), 1);
        assert_eq!(rig.fake.take_calls(), []);
    }

    #[gpui::test]
    fn toggling_while_loading_cancels_the_transfer(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.toggle(cx, 1, "f1");
        rig.commands();
        rig.toggle(cx, 1, "f1");
        assert_eq!(rig.phase(cx, 1), VoicePhase::Idle);
        assert_eq!(rig.commands(), [Command::CancelDownload(playback("f1"))]);

        // The core's answer, and a copy that finished before the cancel took effect, change nothing.
        rig.apply(cx, ClientEvent::DownloadCancelled(playback("f1")));
        assert_eq!(rig.phase(cx, 1), VoicePhase::Idle);
        rig.saved(cx, "f1");
        assert_eq!(rig.phase(cx, 1), VoicePhase::Idle);
        assert_eq!(rig.fake.take_calls(), []);
    }

    #[gpui::test]
    fn pause_keeps_the_position_and_resume_continues(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.playing(cx, 1, "f1");
        rig.fake.set_position(SECS(3));
        wait(cx, 200);
        assert_eq!(rig.view(cx, 1).position, SECS(3));

        rig.toggle(cx, 1, "f1");
        let paused = rig.view(cx, 1);
        assert_eq!(paused.phase, VoicePhase::Paused);
        assert_eq!(paused.position, SECS(3));
        assert_eq!(rig.fake.take_calls(), [Call::Pause]);

        rig.fake.set_position(SECS(8));
        wait(cx, 1_000);
        assert_eq!(
            rig.view(cx, 1),
            paused,
            "a paused view does not follow the output"
        );

        rig.toggle(cx, 1, "f1");
        assert_eq!(rig.phase(cx, 1), VoicePhase::Playing);
        assert_eq!(rig.fake.take_calls(), [Call::Play], "resumed, not reopened");
        rig.fake.set_position(SECS(4));
        wait(cx, 200);
        assert_eq!(rig.view(cx, 1).position, SECS(4));
        assert_eq!(rig.commands(), []);
    }

    #[gpui::test]
    fn starting_another_message_stops_the_first(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.playing(cx, 1, "f1");
        rig.fake.set_position(SECS(5));
        wait(cx, 200);
        let second = rig.saved(cx, "f2");

        rig.toggle(cx, 2, "f2");
        assert_eq!(rig.view(cx, 1), idle_after_open(), "the first is idle at 0");
        assert_eq!(rig.phase(cx, 2), VoicePhase::Playing);
        assert_eq!(
            rig.fake.take_calls(),
            [Call::Stop, Call::Open(second), Call::Play]
        );
        assert_eq!(rig.fake.created(), 1, "one output, reused");

        // Starting another while the first still loads cancels its transfer.
        rig.toggle(cx, 3, "f3");
        rig.commands();
        rig.toggle(cx, 4, "f4");
        assert_eq!(rig.view(cx, 3), VoiceView::default());
        assert_eq!(
            rig.commands(),
            [
                Command::CancelDownload(playback("f3")),
                Command::Download {
                    media: playback("f4"),
                    name: Some("v.ogg".into()),
                },
            ]
        );
    }

    #[gpui::test]
    fn a_message_of_the_same_file_takes_over_the_pending_load(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.toggle(cx, 1, "f1");
        assert_eq!(rig.commands().len(), 1);

        // Two messages, one media id: the transfer is handed over, not cancelled and restarted.
        rig.toggle(cx, 2, "f1");
        assert_eq!(rig.view(cx, 1), VoiceView::default());
        assert_eq!(rig.phase(cx, 2), VoicePhase::Loading);
        assert_eq!(rig.commands(), [], "no cancel, no second download");

        let path = rig.saved(cx, "f1");
        assert_eq!(rig.phase(cx, 2), VoicePhase::Playing);
        assert_eq!(
            rig.view(cx, 1),
            VoiceView::default(),
            "the first stays idle"
        );
        assert_eq!(rig.fake.take_calls(), [Call::Open(path), Call::Play]);

        // The new owner cancels like any loading message.
        rig.player.update(cx, |p, cx| p.stop(cx));
        rig.toggle(cx, 1, "f2");
        rig.commands();
        rig.toggle(cx, 2, "f2");
        assert_eq!(rig.phase(cx, 2), VoicePhase::Loading);
        rig.toggle(cx, 2, "f2");
        assert_eq!(rig.commands(), [Command::CancelDownload(playback("f2"))]);
        assert_eq!(rig.phase(cx, 2), VoicePhase::Idle);
    }

    #[gpui::test]
    fn playing_again_before_the_cancel_is_confirmed_waits_for_it(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.toggle(cx, 1, "f1");
        rig.commands();
        rig.toggle(cx, 1, "f1");
        assert_eq!(rig.commands(), [Command::CancelDownload(playback("f1"))]);

        // The store still holds the cancelled transfer as running: nothing can be asked yet.
        rig.toggle(cx, 1, "f1");
        assert_eq!(rig.phase(cx, 1), VoicePhase::Loading);
        assert_eq!(rig.commands(), [], "no download while the cancel is out");

        rig.apply(cx, ClientEvent::DownloadCancelled(playback("f1")));
        assert_eq!(
            rig.phase(cx, 1),
            VoicePhase::Loading,
            "the restart is not lost"
        );
        assert_eq!(
            rig.commands(),
            [Command::Download {
                media: playback("f1"),
                name: Some("v.ogg".into()),
            }]
        );
        let path = rig.saved(cx, "f1");
        assert_eq!(rig.phase(cx, 1), VoicePhase::Playing);
        assert_eq!(rig.fake.take_calls(), [Call::Open(path), Call::Play]);

        // Another message of the same file, started after a stop: the same wait.
        rig.player.update(cx, |p, cx| p.stop(cx));
        rig.apply(
            cx,
            ClientEvent::DownloadProgress {
                media: playback("f2"),
                received: 1,
                total: None,
            },
        );
        rig.toggle(cx, 3, "f2");
        rig.toggle(cx, 3, "f2");
        rig.commands();
        rig.toggle(cx, 4, "f2");
        assert_eq!(rig.commands(), [], "still waiting for the confirmation");
        rig.apply(cx, ClientEvent::DownloadCancelled(playback("f2")));
        assert_eq!(rig.phase(cx, 4), VoicePhase::Loading);
        assert_eq!(
            rig.commands().len(),
            1,
            "one download for the message that waits"
        );
    }

    #[gpui::test]
    fn a_copy_that_arrives_instead_of_the_cancel_plays_the_waiting_message(
        cx: &mut TestAppContext,
    ) {
        let rig = Rig::new(cx);
        rig.toggle(cx, 1, "f1");
        rig.toggle(cx, 1, "f1");
        rig.toggle(cx, 1, "f1");
        rig.commands();

        // The transfer finished before the cancel took effect.
        let path = rig.saved(cx, "f1");
        assert_eq!(rig.phase(cx, 1), VoicePhase::Playing);
        assert_eq!(rig.fake.take_calls(), [Call::Open(path), Call::Play]);
        assert_eq!(rig.commands(), []);

        // The cancel is settled for good: a later start downloads at once.
        rig.player.update(cx, |p, cx| p.stop(cx));
        rig.apply(
            cx,
            ClientEvent::DownloadFailed {
                media: playback("f1"),
                reason: DownloadFailure::Network,
            },
        );
        rig.toggle(cx, 1, "f1");
        assert_eq!(rig.commands().len(), 1);
    }

    #[gpui::test]
    fn the_measured_length_outlives_the_end_a_stop_and_another_message(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        let idle = |total| VoiceView {
            phase: VoicePhase::Idle,
            position: Duration::ZERO,
            total,
        };
        assert_eq!(
            rig.view(cx, 1),
            idle(None),
            "unknown until the file is opened"
        );

        rig.fake.open_returns(Ok(SECS(37)));
        rig.playing(cx, 1, "f1");
        rig.fake.finish();
        wait(cx, 200);
        assert_eq!(
            rig.view(cx, 1),
            idle(Some(SECS(37))),
            "kept after the natural end"
        );

        // Loading again shows it too, and a stop keeps it; another message is not affected.
        rig.fake.open_returns(Ok(SECS(5)));
        rig.saved(cx, "f2");
        rig.toggle(cx, 2, "f2");
        assert_eq!(rig.view(cx, 2).total, Some(SECS(5)));
        assert_eq!(rig.view(cx, 1), idle(Some(SECS(37))));
        rig.player.update(cx, |p, cx| p.stop(cx));
        assert_eq!(rig.view(cx, 2), idle(Some(SECS(5))));
        assert_eq!(rig.view(cx, 3), idle(None));
    }

    #[gpui::test]
    fn another_account_stops_the_sound_and_clears_what_the_player_knew(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.playing(cx, 1, "f1");
        rig.apply(cx, ClientEvent::AccountSwitched);
        assert_eq!(rig.view(cx, 1), VoiceView::default(), "idle and no length");
        assert_eq!(rig.fake.take_calls(), [Call::Stop]);
        let reads = rig.fake.position_reads();
        wait(cx, 1_000);
        assert_eq!(rig.fake.position_reads(), reads, "the timer ended");

        // A load in flight, and a cancel that was never confirmed: nothing is carried over.
        rig.toggle(cx, 2, "f2");
        rig.toggle(cx, 2, "f2");
        rig.toggle(cx, 2, "f2");
        assert_eq!(rig.phase(cx, 2), VoicePhase::Loading);
        rig.commands();
        rig.apply(cx, ClientEvent::AccountSwitched);
        assert_eq!(rig.view(cx, 2), VoiceView::default());
        rig.toggle(cx, 2, "f2");
        assert_eq!(rig.phase(cx, 2), VoicePhase::Loading);
        assert_eq!(
            rig.commands(),
            [Command::Download {
                media: playback("f2"),
                name: Some("v.ogg".into()),
            }],
            "a start after the switch asks at once, no cancel is awaited"
        );
    }

    #[gpui::test]
    fn the_end_returns_to_idle_and_replays_from_the_start(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        let path = rig.playing(cx, 1, "f1");
        rig.fake.set_position(SECS(9));
        wait(cx, 200);
        rig.fake.finish();
        wait(cx, 200);
        assert_eq!(rig.view(cx, 1), idle_after_open());
        assert_eq!(rig.fake.take_calls(), [Call::Stop]);

        let reads = rig.fake.position_reads();
        wait(cx, 2_000);
        assert_eq!(
            rig.fake.position_reads(),
            reads,
            "the timer ended with the playback"
        );

        rig.toggle(cx, 1, "f1");
        assert_eq!(rig.phase(cx, 1), VoicePhase::Playing);
        assert_eq!(rig.view(cx, 1).position, Duration::ZERO);
        assert_eq!(rig.fake.take_calls(), [Call::Open(path), Call::Play]);
        assert_eq!(rig.commands(), [], "the copy is reused");
    }

    #[gpui::test]
    fn each_open_error_gives_its_failure_and_the_next_click_retries(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.saved(cx, "f1");
        let cases = [
            (OpenError::Unsupported, PlayFailure::Unsupported),
            (OpenError::NoOutput, PlayFailure::NoOutput),
            (OpenError::Io, PlayFailure::Download(DownloadFailure::Disk)),
        ];
        for (error, failure) in cases {
            rig.fake.open_returns(Err(error));
            rig.toggle(cx, 1, "f1");
            assert_eq!(rig.view(cx, 1), {
                VoiceView {
                    phase: VoicePhase::Failed(failure),
                    ..VoiceView::default()
                }
            });
        }
        // A `play` that fails after a good `open` is the same kind of failure and releases the output.
        rig.fake.open_returns(Ok(SECS(10)));
        rig.fake.play_returns(Err(OpenError::NoOutput));
        rig.fake.take_calls();
        rig.toggle(cx, 1, "f1");
        assert_eq!(rig.phase(cx, 1), VoicePhase::Failed(PlayFailure::NoOutput));
        assert_eq!(
            rig.fake.take_calls().last(),
            Some(&Call::Stop),
            "a failed start releases the output"
        );

        rig.fake.play_returns(Ok(()));
        rig.toggle(cx, 1, "f1");
        assert_eq!(rig.phase(cx, 1), VoicePhase::Playing, "the retry works");
        assert_eq!(
            rig.commands(),
            [],
            "a retry of an open failure does not fetch again"
        );
    }

    #[gpui::test]
    fn each_download_failure_gives_its_failure(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        let all = [
            DownloadFailure::Offline,
            DownloadFailure::NotFound,
            DownloadFailure::Destination,
            DownloadFailure::Disk,
            DownloadFailure::Network,
            DownloadFailure::Other,
        ];
        for reason in all {
            rig.toggle(cx, 1, "f1");
            assert_eq!(
                rig.phase(cx, 1),
                VoicePhase::Loading,
                "{reason:?}: the click retries"
            );
            rig.apply(
                cx,
                ClientEvent::DownloadFailed {
                    media: playback("f1"),
                    reason,
                },
            );
            assert_eq!(
                rig.phase(cx, 1),
                VoicePhase::Failed(PlayFailure::Download(reason))
            );
        }
        assert_eq!(rig.fake.take_calls(), []);
    }

    #[gpui::test]
    fn without_a_factory_there_is_no_output_and_no_transfer(cx: &mut TestAppContext) {
        let rig = Rig::without_output(cx);
        rig.toggle(cx, 1, "f1");
        assert_eq!(rig.phase(cx, 1), VoicePhase::Failed(PlayFailure::NoOutput));
        assert_eq!(rig.commands(), []);
        assert_eq!(rig.fake.created(), 0);
    }

    #[gpui::test]
    fn the_timer_stops_when_not_playing_and_a_stale_tick_is_ignored(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.playing(cx, 1, "f1");
        rig.fake.set_position(SECS(1));
        wait(cx, 200);
        assert_eq!(rig.fake.position_reads(), 1, "one read per tick");
        let before_pause = rig.player.read_with(cx, |p, _| p.generation);

        rig.toggle(cx, 1, "f1");
        let reads = rig.fake.position_reads();
        wait(cx, 2_000);
        assert_eq!(rig.fake.position_reads(), reads, "no tick while paused");

        // Paused and resumed inside one tick: the old timer's tick is stale, the new one reads.
        rig.toggle(cx, 1, "f1");
        let reads = rig.fake.position_reads();
        rig.fake.set_position(SECS(2));
        let stale = rig.player.update(cx, |p, cx| p.tick(before_pause, cx));
        assert!(!stale, "a tick of an older stretch ends its timer");
        assert_eq!(
            rig.fake.position_reads(),
            reads,
            "and does not read the output"
        );
        assert_eq!(rig.view(cx, 1).position, SECS(1));
        wait(cx, 200);
        assert_eq!(rig.view(cx, 1).position, SECS(2));
        assert_eq!(
            rig.fake.position_reads(),
            reads + 1,
            "exactly one timer runs"
        );
    }

    #[gpui::test]
    fn the_session_ending_stops_the_playback(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.playing(cx, 1, "f1");
        rig.apply(cx, ClientEvent::SessionExpired);
        assert_eq!(rig.view(cx, 1), VoiceView::default());
        assert_eq!(rig.fake.take_calls(), [Call::Stop]);

        let rig = Rig::new(cx);
        rig.toggle(cx, 1, "f1");
        assert_eq!(rig.phase(cx, 1), VoicePhase::Loading);
        rig.apply(cx, ClientEvent::SignedOut);
        assert_eq!(rig.view(cx, 1), VoiceView::default());
    }

    #[gpui::test]
    fn stop_returns_to_idle_at_zero(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.playing(cx, 1, "f1");
        rig.fake.set_position(SECS(4));
        wait(cx, 200);
        rig.player.update(cx, |p, cx| p.stop(cx));
        assert_eq!(rig.view(cx, 1), idle_after_open());
        assert_eq!(rig.fake.take_calls(), [Call::Stop]);

        let reads = rig.fake.position_reads();
        wait(cx, 1_000);
        assert_eq!(rig.fake.position_reads(), reads);

        // Nothing is playing: no call.
        rig.player.update(cx, |p, cx| p.stop(cx));
        assert_eq!(rig.fake.take_calls(), []);
    }

    #[gpui::test]
    fn active_names_the_message_that_is_not_idle(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        let active = |rig: &Rig, cx: &mut TestAppContext| {
            rig.player.read_with(cx, |p, _| p.active().cloned())
        };
        assert_eq!(active(&rig, cx), None);
        rig.toggle(cx, 1, "f1");
        assert_eq!(active(&rig, cx), Some(key(1)), "loading counts");
        rig.saved(cx, "f1");
        assert_eq!(active(&rig, cx), Some(key(1)), "playing counts");
        rig.player.update(cx, |p, cx| p.stop(cx));
        assert_eq!(active(&rig, cx), None);
    }

    #[gpui::test]
    fn closing_the_window_stops_the_sound(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.playing(cx, 1, "f1");
        let Rig { player, fake, .. } = rig;
        drop(player);
        cx.update(|_| {});
        cx.run_until_parked();
        assert_eq!(fake.take_calls(), [Call::Stop]);
    }

    #[test]
    fn every_failure_has_a_short_label() {
        use DownloadFailure::*;
        let labels = [
            (PlayFailure::Download(Offline), "Нет сети"),
            (PlayFailure::Download(Network), "Нет сети"),
            (PlayFailure::Download(NotFound), "Файл недоступен"),
            (PlayFailure::Download(Disk), "Файл недоступен"),
            (PlayFailure::Download(Destination), "Ошибка"),
            (PlayFailure::Download(Other), "Ошибка"),
            (PlayFailure::Unsupported, "Формат не поддерживается"),
            (PlayFailure::NoOutput, "Нет вывода звука"),
        ];
        for (failure, label) in labels {
            assert_eq!(failure_label(failure), label, "{failure:?}");
        }
    }

    fn fetch(file: &str, name: &str) -> Command {
        Command::Download {
            media: playback(file),
            name: Some(name.into()),
        }
    }

    fn opens(path: &Path) -> PlayerEvent {
        PlayerEvent::OpenExternal(path.to_path_buf())
    }

    // ---- video (PB 5.7) ----

    #[gpui::test]
    fn a_video_is_fetched_and_opened_on_saved(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.open(cx, "v1", "x.mp4");
        assert_eq!(rig.video(cx, "v1"), VoicePhase::Loading);
        assert_eq!(rig.commands(), [fetch("v1", "x.mp4")]);
        assert_eq!(rig.events(), []);

        // The copy of another file does not open this video.
        rig.saved_as(cx, "other", "o.mp4");
        assert_eq!(rig.events(), []);
        assert_eq!(rig.video(cx, "v1"), VoicePhase::Loading);

        let path = rig.saved_as(cx, "v1", "hash.mp4");
        assert_eq!(rig.events(), [opens(&path)], "the exact path");
        assert_eq!(rig.video(cx, "v1"), VoicePhase::Idle);
        assert_eq!(rig.commands(), []);
        assert_eq!(rig.fake.created(), 0, "a video needs no audio output");
        assert_eq!(rig.fake.take_calls(), []);
    }

    #[gpui::test]
    fn an_existing_copy_opens_at_once_again_and_offline(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        let path = rig.saved_as(cx, "v1", "hash.mp4");
        rig.apply(cx, ClientEvent::Connection(ConnectionState::Offline));
        rig.open(cx, "v1", "x.mp4");
        assert_eq!(rig.events(), [opens(&path)]);
        assert_eq!(rig.video(cx, "v1"), VoicePhase::Idle);
        assert_eq!(rig.commands(), [], "nothing is fetched");

        rig.open(cx, "v1", "x.mp4");
        assert_eq!(rig.events(), [opens(&path)], "a second play opens it again");
        assert_eq!(rig.commands(), []);
    }

    #[gpui::test]
    fn a_video_copy_that_vanished_from_the_disk_is_fetched_again(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        let path = rig.saved_as(cx, "v1", "hash.mp4");
        std::fs::remove_file(&path).unwrap();
        rig.open(cx, "v1", "x.mp4");
        assert_eq!(rig.events(), []);
        assert_eq!(rig.video(cx, "v1"), VoicePhase::Loading);
        assert_eq!(rig.commands(), [fetch("v1", "x.mp4")]);
    }

    #[gpui::test]
    fn the_name_given_to_open_video_is_not_trusted(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        // The same file was saved earlier for another message, under a name that is no video's.
        let old = rig.saved_as(cx, "v1", "hash.command");
        rig.open(cx, "v1", "x.mp4");
        assert_eq!(rig.events(), [], "the old copy is not opened");
        assert_eq!(rig.video(cx, "v1"), VoicePhase::Loading);
        assert_eq!(
            rig.commands(),
            [fetch("v1", "x.mp4")],
            "a new copy under the video's own name"
        );

        let new = rig.saved_as(cx, "v1", "hash.mp4");
        assert_ne!(old, new);
        assert_eq!(rig.events(), [opens(&new)]);
        assert_eq!(rig.video(cx, "v1"), VoicePhase::Idle);
    }

    #[gpui::test]
    fn a_reused_copy_with_a_bad_extension_is_absent(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        let names = ["hash", "hash.app", "hash.mp4.command", "hash.", ".mp4"];
        for (i, name) in names.into_iter().enumerate() {
            let file = format!("v{i}");
            rig.saved_as(cx, &file, name);
            rig.open(cx, &file, "x.mp4");
            assert_eq!(rig.events(), [], "{name}: not opened");
            assert_eq!(rig.video(cx, &file), VoicePhase::Loading, "{name}");
            assert_eq!(rig.commands(), [fetch(&file, "x.mp4")], "{name}");
        }

        // Whitelisted, in any case.
        for (i, name) in ["hash.MP4", "hash.mov", "hash.MOV"].into_iter().enumerate() {
            let file = format!("w{i}");
            let path = rig.saved_as(cx, &file, name);
            rig.open(cx, &file, "x.mp4");
            assert_eq!(rig.events(), [opens(&path)], "{name}");
            assert_eq!(rig.commands(), [], "{name}");
        }
    }

    #[gpui::test]
    fn a_fresh_copy_that_fails_the_check_is_unsupported(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.open(cx, "v1", "x.mp4");
        rig.commands();
        rig.saved_as(cx, "v1", "hash.app");
        assert_eq!(rig.events(), []);
        assert_eq!(
            rig.video(cx, "v1"),
            VoicePhase::Failed(PlayFailure::Unsupported)
        );

        // The next click does not reuse that copy: it fetches a new one and checks it again.
        rig.open(cx, "v1", "x.mp4");
        assert_eq!(rig.video(cx, "v1"), VoicePhase::Loading);
        assert_eq!(rig.commands(), [fetch("v1", "x.mp4")]);
        let path = rig.saved_as(cx, "v1", "hash2.mp4");
        assert_eq!(rig.events(), [opens(&path)]);
    }

    #[gpui::test]
    fn cancelling_while_loading_stops_the_transfer_and_opens_nothing(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.open(cx, "v1", "x.mp4");
        rig.commands();
        rig.open(cx, "v1", "x.mp4");
        assert_eq!(rig.video(cx, "v1"), VoicePhase::Idle);
        assert_eq!(rig.commands(), [Command::CancelDownload(playback("v1"))]);

        // The core's answer, and a copy that finished before the cancel took effect, open nothing.
        rig.apply(cx, ClientEvent::DownloadCancelled(playback("v1")));
        assert_eq!(rig.video(cx, "v1"), VoicePhase::Idle);
        rig.saved_as(cx, "v1", "hash.mp4");
        assert_eq!(rig.events(), []);
        assert_eq!(rig.video(cx, "v1"), VoicePhase::Idle);
    }

    #[gpui::test]
    fn playing_a_video_again_before_the_cancel_is_confirmed_waits_for_it(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.open(cx, "v1", "x.mp4");
        rig.open(cx, "v1", "x.mp4");
        rig.commands();

        rig.open(cx, "v1", "x.mp4");
        assert_eq!(rig.video(cx, "v1"), VoicePhase::Loading);
        assert_eq!(rig.commands(), [], "no download while the cancel is out");

        rig.apply(cx, ClientEvent::DownloadCancelled(playback("v1")));
        assert_eq!(rig.video(cx, "v1"), VoicePhase::Loading);
        assert_eq!(rig.commands(), [fetch("v1", "x.mp4")]);
        let path = rig.saved_as(cx, "v1", "hash.mp4");
        assert_eq!(rig.events(), [opens(&path)]);
    }

    #[gpui::test]
    fn offline_with_no_usable_copy_fails_without_a_command(cx: &mut TestAppContext) {
        let offline = ClientEvent::Connection(ConnectionState::Offline);
        let rig = Rig::new(cx);
        rig.apply(cx, offline.clone());
        rig.open(cx, "v1", "x.mp4");
        assert_eq!(
            rig.video(cx, "v1"),
            VoicePhase::Failed(PlayFailure::Download(DownloadFailure::Offline))
        );
        assert_eq!(rig.commands(), []);
        assert_eq!(rig.events(), []);

        // Only a copy that fails the check: the same.
        let rig = Rig::new(cx);
        rig.saved_as(cx, "v1", "hash.command");
        rig.apply(cx, offline);
        rig.open(cx, "v1", "x.mp4");
        assert_eq!(
            rig.video(cx, "v1"),
            VoicePhase::Failed(PlayFailure::Download(DownloadFailure::Offline))
        );
        assert_eq!(rig.commands(), []);
        assert_eq!(rig.events(), []);
    }

    #[gpui::test]
    fn each_download_failure_shows_and_the_next_click_retries(cx: &mut TestAppContext) {
        use DownloadFailure::*;
        let rig = Rig::new(cx);
        for reason in [Offline, NotFound, Destination, Disk, Network, Other] {
            rig.open(cx, "v1", "x.mp4");
            assert_eq!(rig.video(cx, "v1"), VoicePhase::Loading, "{reason:?}");
            assert_eq!(rig.commands(), [fetch("v1", "x.mp4")], "{reason:?}");
            rig.apply(
                cx,
                ClientEvent::DownloadFailed {
                    media: playback("v1"),
                    reason,
                },
            );
            assert_eq!(
                rig.video(cx, "v1"),
                VoicePhase::Failed(PlayFailure::Download(reason))
            );
        }
        assert_eq!(rig.events(), []);
    }

    #[gpui::test]
    fn another_account_or_the_end_of_the_session_cancels_a_pending_load(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.open(cx, "v1", "x.mp4");
        rig.open(cx, "v2", "y.mp4");
        rig.apply(
            cx,
            ClientEvent::DownloadFailed {
                media: playback("v2"),
                reason: DownloadFailure::Network,
            },
        );
        rig.commands();
        rig.apply(cx, ClientEvent::AccountSwitched);
        assert_eq!(rig.video(cx, "v1"), VoicePhase::Idle);
        assert_eq!(
            rig.video(cx, "v2"),
            VoicePhase::Idle,
            "a failure is forgotten"
        );

        // Whatever the old account's transfer still delivers opens nothing.
        rig.saved_as(cx, "v1", "hash.mp4");
        assert_eq!(rig.events(), []);

        // A click after the switch asks at once: no cancel is awaited.
        rig.open(cx, "v3", "z.mp4");
        assert_eq!(rig.video(cx, "v3"), VoicePhase::Loading);
        assert_eq!(rig.commands(), [fetch("v3", "z.mp4")]);

        rig.apply(cx, ClientEvent::SignedOut);
        assert_eq!(rig.video(cx, "v3"), VoicePhase::Idle);
        rig.saved_as(cx, "v3", "hash.mp4");
        assert_eq!(rig.events(), []);
    }

    #[gpui::test]
    fn opening_a_video_leaves_a_playing_voice_alone(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        rig.playing(cx, 1, "f1");
        let path = rig.saved_as(cx, "v1", "hash.mp4");
        rig.open(cx, "v1", "x.mp4");
        assert_eq!(rig.events(), [opens(&path)]);
        assert_eq!(rig.phase(cx, 1), VoicePhase::Playing);
        assert_eq!(rig.fake.take_calls(), []);

        // And stopping the voice does not touch a video that is loading.
        rig.open(cx, "v2", "y.mp4");
        rig.player.update(cx, |p, cx| p.stop(cx));
        assert_eq!(rig.video(cx, "v2"), VoicePhase::Loading);
    }

    #[test]
    fn only_the_extension_of_the_actual_path_counts() {
        for yes in ["/a/b/x.mp4", "x.MP4", "/a/x.y.mov", "/a/b.command/x.MOV"] {
            assert!(is_video_path(Path::new(yes)), "{yes}");
        }
        for no in [
            "",
            "/",
            "/a/b.mp4/x",
            "x",
            "x.",
            ".mp4",
            "x.app",
            "x.mp4.command",
            "x.webm",
        ] {
            assert!(!is_video_path(Path::new(no)), "{no:?}");
        }
    }
}
