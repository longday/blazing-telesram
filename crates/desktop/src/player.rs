//! Voice playback: an audio output behind `AudioOutput` and the state machine that drives it.
//!
//! `Player` owns no UI. Views read `voice(key)` and call `toggle_voice`; the playback copy of the
//! file comes from the store (`ChatStore::download` with the `Playback` kind), the sound from the
//! `AudioOutput` the registered `AudioFactory` makes. No factory registered means no sound output.
//! Nothing here logs: no path, file name or message is ever written to a log line.

use crate::store::{ChatStore, DownloadState, SessionState, StoreEvent};
use bt_chat_core::model::{ChatId, DownloadFailure, MediaRef, MessageTs};
use gpui::{App, Context, Entity, Global, Subscription};
use std::path::Path;
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

/// What a bubble shows. `total` is the length of the opened file, `None` until the file is open
/// (the bubble then shows the duration the message carries) and after the playback ended.
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
    _store_events: Subscription,
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
            _store_events: subscription,
        }
    }

    pub fn voice(&self, key: &VoiceKey) -> VoiceView {
        match &self.current {
            Some(c) if &c.key == key => VoiceView {
                phase: c.phase,
                position: c.position,
                total: c.total,
            },
            _ => VoiceView::default(),
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
            VoicePhase::Loading => {
                let store = self.store.read(cx);
                if matches!(
                    store.download_state(&current.playback),
                    Some(DownloadState::Running { .. })
                ) {
                    store.cancel_download(&current.playback);
                    if !self.cancelling.contains(&current.playback) {
                        self.cancelling.push(current.playback);
                    }
                }
            }
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
            }
            StoreEvent::Session if self.store.read(cx).session() != SessionState::Active => {
                self.stop(cx);
                self.cancelling.clear();
            }
            _ => {}
        }
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
    use std::path::PathBuf;

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
            let rig = Self {
                store,
                player,
                commands,
                fake,
                dir: tempfile::tempdir().unwrap(),
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
            let path = self.dir.path().join(format!("{file}.ogg"));
            std::fs::write(&path, b"ogg").unwrap();
            self.apply(
                cx,
                ClientEvent::Downloaded {
                    media: playback(file),
                    path: path.clone(),
                },
            );
            path
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
        assert_eq!(
            rig.view(cx, 1),
            VoiceView::default(),
            "the first is idle at 0"
        );
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
    fn the_end_returns_to_idle_and_replays_from_the_start(cx: &mut TestAppContext) {
        let rig = Rig::new(cx);
        let path = rig.playing(cx, 1, "f1");
        rig.fake.set_position(SECS(9));
        wait(cx, 200);
        rig.fake.finish();
        wait(cx, 200);
        assert_eq!(rig.view(cx, 1), VoiceView::default());
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
        assert_eq!(rig.view(cx, 1), VoiceView::default());
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
}
