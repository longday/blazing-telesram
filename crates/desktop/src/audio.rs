//! The real `AudioOutput`: Core Audio through `objc2-avf-audio`.
//!
//! One `AVAudioFile` is read by an `AVAudioPlayerNode` in an `AVAudioEngine`. The simpler
//! `AVAudioPlayer` fails silently on 7 of the 17 voice samples of the research (web-recorded Ogg
//! Opus: `play` returns false with no error); the engine path plays every one of them. The facts
//! this file relies on are in `openspec/changes/add-media-playback/research.md` (R4, R5):
//!
//! - The error codes of Core Audio do not tell a missing file from a bad one, so `Io` is decided by
//!   `std::fs` before the call and every error of Core Audio is `Unsupported`. A file that opens
//!   with zero frames (a truncated stream) is `Unsupported` too.
//! - `scheduleFile` comes before `play`. The completion handler (type `DataPlayedBack`) runs on
//!   another thread and fires at the natural end and also at our own `stop`, so it is only the end
//!   when no `stop` and no later `scheduleFile` happened since it was scheduled (`generation`).
//! - The node keeps counting past the end of the file and its clock restarts at 0 only after a
//!   `stop`, so every run starts with one. While paused the render time is `None`, so the position
//!   of the last read is kept.
//! - A failing `startAndReturnError` is `NoOutput`. Playing on an engine that is not running raises
//!   an Objective-C exception, so `play` never calls the node then.

use crate::player::{AudioOutput, OpenError};
use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_avf_audio::{
    AVAudioEngine, AVAudioFile, AVAudioPlayerNode, AVAudioPlayerNodeCompletionCallbackType,
};
use objc2_foundation::{NSString, NSURL};
use std::cell::Cell;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The `AudioFactory` of the app.
pub fn output() -> Box<dyn AudioOutput> {
    Box::new(CoreAudio::default())
}

/// What the node is doing, as far as this output knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Run {
    /// Nothing is scheduled; the position is 0.
    Ready,
    Playing,
    Paused,
}

/// The opened file with the engine that plays it.
struct Track {
    file: Retained<AVAudioFile>,
    engine: Retained<AVAudioEngine>,
    node: Retained<AVAudioPlayerNode>,
    length: Duration,
}

struct CoreAudio {
    track: Option<Track>,
    run: Run,
    /// The position of the last read; kept across `pause`, when the render time is not available.
    last: Cell<Duration>,
    /// Bumped by `stop` and by every `scheduleFile`.
    generation: Arc<AtomicU64>,
    /// The newest generation whose completion handler fired (0: none; generations start at 1).
    /// The handler runs on another thread and the handlers of a `stop` and of the run after it
    /// can arrive in either order, so the number only grows (`record_end`); a handler of an older
    /// schedule then neither counts as an end nor undoes a newer one.
    ended: Arc<AtomicU64>,
}

impl Default for CoreAudio {
    fn default() -> Self {
        Self {
            track: None,
            run: Run::Ready,
            last: Cell::new(Duration::ZERO),
            generation: Arc::new(AtomicU64::new(1)),
            ended: Arc::new(AtomicU64::new(0)),
        }
    }
}

/// Called by the completion handler of the schedule of generation `generation`.
fn record_end(ended: &AtomicU64, generation: u64) {
    ended.fetch_max(generation, Ordering::SeqCst);
}

impl CoreAudio {
    /// Schedules the whole file on a node that has just been stopped, then plays it.
    fn start(&mut self) -> Result<(), OpenError> {
        let track = self.track.as_ref().ok_or(OpenError::Unsupported)?;
        // The handler of an earlier schedule, which `node.stop()` fires, carries an older number.
        let mine = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        // SAFETY: plain messages to objects this struct owns; the block is copied by Core Audio
        // and its pointer is valid for the call.
        unsafe {
            track.node.stop();
            if !track.engine.isRunning() {
                track
                    .engine
                    .startAndReturnError()
                    .map_err(|_| OpenError::NoOutput)?;
            }
            let ended = Arc::clone(&self.ended);
            let handler = RcBlock::new(move |_: AVAudioPlayerNodeCompletionCallbackType| {
                // Only the number is stored; `finished` compares it with the current generation.
                record_end(&ended, mine);
            });
            track
                .node
                .scheduleFile_atTime_completionCallbackType_completionHandler(
                    &track.file,
                    None,
                    AVAudioPlayerNodeCompletionCallbackType::DataPlayedBack,
                    RcBlock::as_ptr(&handler),
                );
            track.node.play();
        }
        self.run = Run::Playing;
        self.last.set(Duration::ZERO);
        Ok(())
    }

    /// The node's clock in the file, `None` while it cannot be read (not rendered yet, paused).
    fn clock(&self, track: &Track) -> Option<Duration> {
        // SAFETY: plain messages to objects this struct owns.
        let seconds = unsafe {
            let rendered = track.node.lastRenderTime()?;
            let time = track.node.playerTimeForNodeTime(&rendered)?;
            time.sampleTime() as f64 / time.sampleRate()
        };
        // Negative just after the start, and past the end the node keeps counting.
        seconds
            .is_finite()
            .then(|| Duration::from_secs_f64(seconds.clamp(0.0, track.length.as_secs_f64())))
    }
}

impl AudioOutput for CoreAudio {
    fn open(&mut self, path: &Path) -> Result<Duration, OpenError> {
        self.stop();
        self.track = None;
        // Core Audio answers a missing file and a bad one with the same codes.
        let readable = std::fs::File::open(path)
            .and_then(|f| f.metadata())
            .is_ok_and(|m| m.is_file());
        if !readable {
            return Err(OpenError::Io);
        }
        let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().ok_or(OpenError::Io)?));
        // SAFETY: plain messages to objects created here.
        let track = unsafe {
            let file = AVAudioFile::initForReading_error(AVAudioFile::alloc(), &url)
                .map_err(|_| OpenError::Unsupported)?;
            let format = file.processingFormat();
            let (frames, rate) = (file.length(), format.sampleRate());
            if frames <= 0 || rate <= 0.0 {
                return Err(OpenError::Unsupported);
            }
            let engine = AVAudioEngine::new();
            let node = AVAudioPlayerNode::new();
            engine.attachNode(&node);
            engine.connect_to_format(&node, &engine.mainMixerNode(), Some(&format));
            Track {
                file,
                engine,
                node,
                length: Duration::from_secs_f64(frames as f64 / rate),
            }
        };
        let length = track.length;
        self.track = Some(track);
        Ok(length)
    }

    fn play(&mut self) -> Result<(), OpenError> {
        let Some(track) = self.track.as_ref() else {
            return Err(OpenError::Unsupported);
        };
        match self.run {
            Run::Playing if !self.finished() => Ok(()),
            Run::Paused => {
                // SAFETY: as in `start`; the node is touched only while the engine runs.
                unsafe {
                    if !track.engine.isRunning() {
                        return Err(OpenError::NoOutput);
                    }
                    track.node.play();
                }
                self.run = Run::Playing;
                Ok(())
            }
            // Never played, stopped, or ended: from the start.
            Run::Ready | Run::Playing => self.start(),
        }
    }

    fn pause(&mut self) {
        if self.run != Run::Playing {
            return;
        }
        self.last.set(self.position());
        if let Some(track) = self.track.as_ref() {
            // SAFETY: a plain message to an object this struct owns.
            unsafe { track.node.pause() };
        }
        self.run = Run::Paused;
    }

    fn stop(&mut self) {
        // First, so that the handler this `stop` fires is not an end.
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(track) = self.track.as_ref() {
            // SAFETY: plain messages to objects this struct owns.
            unsafe {
                track.node.stop();
                track.engine.stop();
            }
        }
        self.run = Run::Ready;
        self.last.set(Duration::ZERO);
    }

    fn position(&self) -> Duration {
        if let (Run::Playing, Some(track)) = (self.run, self.track.as_ref())
            && let Some(position) = self.clock(track)
        {
            self.last.set(position);
        }
        self.last.get()
    }

    fn finished(&self) -> bool {
        self.ended.load(Ordering::SeqCst) == self.generation.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mono 16-bit PCM WAV of `frames` frames of silence at `rate`.
    fn wav(frames: u32, rate: u32) -> Vec<u8> {
        let data = frames * 2;
        let mut bytes = b"RIFF".to_vec();
        bytes.extend((36 + data).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes()); // PCM
        bytes.extend(1u16.to_le_bytes()); // mono
        bytes.extend(rate.to_le_bytes());
        bytes.extend((rate * 2).to_le_bytes());
        bytes.extend(2u16.to_le_bytes()); // block align
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(data.to_le_bytes());
        bytes.extend(std::iter::repeat_n(0u8, data as usize));
        bytes
    }

    fn open(bytes: &[u8]) -> (CoreAudio, Result<Duration, OpenError>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.wav");
        std::fs::write(&path, bytes).unwrap();
        let mut output = CoreAudio::default();
        let opened = output.open(&path);
        (output, opened)
    }

    #[test]
    fn a_late_handler_of_an_older_schedule_never_hides_the_end() {
        let output = CoreAudio::default();
        assert!(!output.finished(), "a new output has not ended");
        // A run (generation 2, after a `stop` had made it 1) schedules and ends first; the handler
        // that the `stop` fired arrives afterwards, from another thread.
        output.generation.fetch_add(2, Ordering::SeqCst);
        let current = output.generation.load(Ordering::SeqCst);
        record_end(&output.ended, current);
        assert!(output.finished());
        record_end(&output.ended, current - 1);
        assert!(
            output.finished(),
            "the stale handler came second and changed nothing"
        );
    }

    #[test]
    fn a_handler_of_an_older_schedule_alone_is_not_an_end() {
        let output = CoreAudio::default();
        output.generation.fetch_add(2, Ordering::SeqCst);
        let current = output.generation.load(Ordering::SeqCst);
        record_end(&output.ended, current - 1);
        assert!(!output.finished());
        record_end(&output.ended, current);
        assert!(output.finished());
        output.generation.fetch_add(1, Ordering::SeqCst);
        assert!(!output.finished(), "a `stop` after the end voids it");
    }

    #[test]
    fn a_path_that_does_not_exist_is_io() {
        let dir = tempfile::tempdir().unwrap();
        let mut output = CoreAudio::default();
        assert_eq!(
            output.open(&dir.path().join("missing.ogg")),
            Err(OpenError::Io)
        );
    }

    #[test]
    fn a_folder_is_io() {
        let dir = tempfile::tempdir().unwrap();
        let mut output = CoreAudio::default();
        assert_eq!(output.open(dir.path()), Err(OpenError::Io));
    }

    #[test]
    fn an_empty_file_is_unsupported() {
        assert_eq!(open(b"").1, Err(OpenError::Unsupported));
    }

    #[test]
    fn garbage_is_unsupported() {
        assert_eq!(open(b"garbage").1, Err(OpenError::Unsupported));
        assert_eq!(open(&[0xA5; 4096]).1, Err(OpenError::Unsupported));
    }

    #[test]
    fn a_file_with_no_frames_is_unsupported() {
        assert_eq!(open(&wav(0, 8000)).1, Err(OpenError::Unsupported));
    }

    #[test]
    fn a_wav_opens_with_its_length() {
        let (output, opened) = open(&wav(4000, 8000));
        assert_eq!(opened, Ok(Duration::from_millis(500)));
        assert_eq!(output.position(), Duration::ZERO);
        assert!(!output.finished(), "nothing was played");
        let (_, opened) = open(&wav(48_000 * 3, 48_000));
        assert_eq!(opened, Ok(Duration::from_secs(3)));
    }

    #[test]
    fn a_failed_open_drops_the_previous_file() {
        let (mut output, opened) = open(&wav(4000, 8000));
        assert!(opened.is_ok());
        assert_eq!(
            output.open(Path::new("/nonexistent/x.ogg")),
            Err(OpenError::Io)
        );
        assert_eq!(
            output.play(),
            Err(OpenError::Unsupported),
            "no file is open"
        );
    }

    #[test]
    fn controls_without_a_file_do_nothing() {
        let mut output = CoreAudio::default();
        output.pause();
        output.stop();
        assert_eq!(output.position(), Duration::ZERO);
        assert!(!output.finished());
        assert_eq!(output.play(), Err(OpenError::Unsupported));
    }

    #[test]
    fn stop_without_a_play_is_not_an_end() {
        let (mut output, opened) = open(&wav(4000, 8000));
        assert!(opened.is_ok());
        output.stop();
        output.stop();
        assert!(!output.finished());
        assert_eq!(output.position(), Duration::ZERO);
    }
}
