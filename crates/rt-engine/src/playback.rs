//! The player thread: renders replays from the speech buffer into an output stream, opened per replay.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use rt_core::resample::LinearResampler;
use rt_core::{Player, PlayerConfig};

use crate::EngineConfig;
use crate::beep::beep;
use crate::duck::DuckCmd;
use crate::output::{FeedWriter, OutputBackend, OutputFeed, OutputStream};
use crate::status::{PlaybackState, StatusHub};
use crate::worker::{Core, SharedCore, lock};

/// How often the player tops up the output while a stream is open.
const TICK: Duration = Duration::from_millis(5);
/// How much audio the player renders ahead of the output. More survives a late tick; less makes a speed change audible
/// sooner.
const AHEAD: Duration = Duration::from_millis(80);
/// Samples the output's queue holds: half a second at 384 kHz, room for what's ahead and a rendered block, or the
/// beep, at any device rate.
const QUEUE_LEN: usize = 192_000;
/// The player renders blocks of this much captured audio.
const BLOCK: Duration = Duration::from_millis(10);
/// How often `behind` is updated in the status during a replay.
const STATUS_EVERY: Duration = Duration::from_millis(500);
/// Silence played after the beep.
const BEEP_TAIL: Duration = Duration::from_millis(50);
/// A stopped stream stays open this long, so its fade-out plays.
const CLOSE_AFTER_STOP: Duration = Duration::from_millis(50);

pub(crate) enum PlayCmd {
    Replay,
    Stop,
    ToggleCatchUp,
    SetConfig(EngineConfig),
    /// The buffer is being wiped (pause, another target): stop at once. The duck thread restores the volume itself.
    Wipe,
    Shutdown,
}

/// An open output stream.
struct Output {
    stream: Box<dyn OutputStream>,
    writer: FeedWriter,
    /// The rate the player renders at.
    capture_rate: Option<u32>,
    /// From the capture rate to the device rate; `None` if they're the same.
    resampler: Option<LinearResampler>,
    /// Nothing more is coming: the stream closes once what's queued has played.
    draining: bool,
}

pub(crate) struct PlayerThread {
    core: SharedCore,
    backend: Arc<dyn OutputBackend>,
    duck: Sender<DuckCmd>,
    status: Arc<StatusHub>,
    config: EngineConfig,
    /// The player, and the recorder generation it was made for.
    player: Option<(Player, u64)>,
    catching_up: bool,
    /// This replay ducked the app, so its end restores it.
    ducked: bool,
    output: Option<Output>,
    /// A stopped stream fading out, closed at the given instant.
    closing: Option<(Box<dyn OutputStream>, Instant)>,
    block: Vec<f32>,
    resampled: Vec<f32>,
    last_status: Instant,
}

impl PlayerThread {
    pub(crate) fn new(
        core: SharedCore,
        backend: Arc<dyn OutputBackend>,
        duck: Sender<DuckCmd>,
        status: Arc<StatusHub>,
        config: EngineConfig,
    ) -> Self {
        Self {
            core,
            backend,
            duck,
            status,
            config,
            player: None,
            catching_up: false,
            ducked: false,
            output: None,
            closing: None,
            block: Vec::new(),
            resampled: Vec::new(),
            last_status: Instant::now(),
        }
    }

    pub(crate) fn run(mut self, commands: Receiver<PlayCmd>) {
        loop {
            let timeout = if self.output.is_some() || self.closing.is_some() {
                TICK
            } else {
                STATUS_EVERY
            };
            match commands.recv_timeout(timeout) {
                Ok(PlayCmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(command) => self.handle(command),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.fill();
            if self.closing.as_ref().is_some_and(|(_, at)| *at <= Instant::now()) {
                self.closing = None;
            }
            if self.is_playing() && self.last_status.elapsed() >= STATUS_EVERY {
                self.report_behind(true);
            }
        }
        self.stop_now();
    }

    fn handle(&mut self, command: PlayCmd) {
        match command {
            PlayCmd::Replay => self.replay(),
            PlayCmd::Stop => self.stop(),
            PlayCmd::ToggleCatchUp => self.toggle_catch_up(),
            PlayCmd::SetConfig(config) => {
                self.config = config;
                let speed = self.speed();
                if let Some((player, _)) = &mut self.player {
                    player.set_speed(speed);
                }
            }
            PlayCmd::Wipe => {
                // The duck thread restores the volume on its own when the target changes or capture pauses.
                self.ducked = false;
                self.stop_now();
                self.player = None;
                self.set_idle();
            }
            PlayCmd::Shutdown => {}
        }
    }

    fn speed(&self) -> f32 {
        if self.catching_up {
            self.config.catch_up_speed
        } else {
            self.config.replay_speed
        }
    }

    fn is_playing(&self) -> bool {
        self.player.as_ref().is_some_and(|(p, _)| p.is_playing())
    }

    /// The player for the recorder's current buffer. A new recorder (another capture format) means a new player.
    fn sync_player(player: &mut Option<(Player, u64)>, core: &Core) {
        let Some(recorder) = &core.recorder else {
            *player = None;
            return;
        };
        if player
            .as_ref()
            .is_none_or(|(_, generation)| *generation != core.generation)
        {
            let rate = recorder.buffer().sample_rate();
            *player = Some((Player::new(rate, PlayerConfig::default()), core.generation));
        }
    }

    fn replay(&mut self) {
        let speed = self.speed();
        let core = lock(&self.core);
        let had_player = self.player.as_ref().map(|(_, g)| *g);
        Self::sync_player(&mut self.player, &core);
        let (Some((player, generation)), Some(recorder)) = (&mut self.player, &core.recorder) else {
            drop(core);
            return self.beep();
        };
        if had_player == Some(*generation) && player.is_playing() {
            // Another press during a replay: one utterance back, at the same speed and duck.
            player.replay(recorder.buffer());
            return;
        }
        player.set_speed(speed);
        if !player.replay(recorder.buffer()) {
            drop(core);
            return self.beep();
        }
        drop(core);

        if !self.open_output() {
            if let Some((player, _)) = &mut self.player {
                player.stop();
            }
            self.catching_up = false;
            return;
        }
        if let Some(output) = &mut self.output {
            output.draining = false;
        }
        if let Some(level) = self.config.duck {
            let _ = self.duck.send(DuckCmd::Duck(level));
            self.ducked = true;
        }
        self.report_behind(false);
    }

    /// Plays the beep that says there's nothing to replay. No ducking.
    fn beep(&mut self) {
        self.catching_up = false;
        if !self.open_output() {
            return;
        }
        if let Some(output) = &mut self.output {
            let rate = output.stream.sample_rate();
            output.writer.write(&beep(rate));
            // Silence after it, so its end is out of the device's buffer when the stream closes.
            output
                .writer
                .write(&vec![0.0; (BEEP_TAIL.as_secs_f64() * f64::from(rate)) as usize]);
            output.draining = true;
        }
    }

    fn toggle_catch_up(&mut self) {
        if !self.is_playing() {
            self.catching_up = true;
            self.replay();
            if !self.is_playing() {
                self.catching_up = false;
            }
            return;
        }
        self.catching_up = !self.catching_up;
        let speed = self.speed();
        if let Some((player, _)) = &mut self.player {
            player.set_speed(speed);
        }
        self.report_behind(false);
    }

    /// Stops the replay: the output fades out, and the volume comes back after the restore delay.
    fn stop(&mut self) {
        self.stop_now();
        self.set_idle();
    }

    fn stop_now(&mut self) {
        if let Some((player, _)) = &mut self.player {
            player.stop();
        }
        if let Some(output) = self.output.take() {
            output.writer.stop();
            self.closing = Some((output.stream, Instant::now() + CLOSE_AFTER_STOP));
        }
        self.block.fill(0.0);
        self.resampled.fill(0.0);
    }

    /// The replay is over: back to replay speed, and the app's volume comes back.
    fn set_idle(&mut self) {
        self.catching_up = false;
        if std::mem::take(&mut self.ducked) {
            let _ = self.duck.send(DuckCmd::Restore);
        }
        self.status.update(|s| {
            s.playback = PlaybackState::Idle;
            s.behind = None;
        });
    }

    /// Makes sure an output is open: the chosen device, else the default. Reports the device, or the error, in the
    /// status. Returns whether one is open.
    fn open_output(&mut self) -> bool {
        let rate = self.capture_rate();
        if self.output.as_ref().is_some_and(|o| o.capture_rate != rate) {
            // Still playing out audio of a capture in another format.
            self.stop_now();
        }
        if self.output.is_some() {
            return true;
        }
        let open = |id: Option<&str>| {
            let (writer, feed) = OutputFeed::new(QUEUE_LEN);
            self.backend.open(id, feed).map(|stream| (stream, writer))
        };
        let (result, fell_back) = match self.config.output_device.as_deref() {
            Some(id) => match open(Some(id)) {
                Ok(opened) => (Ok(opened), false),
                Err(_) => (open(None), true),
            },
            None => (open(None), false),
        };
        match result {
            Ok((stream, writer)) => {
                let name = stream.device_name().to_owned();
                self.status.update(|s| {
                    s.output.device_name = Some(name);
                    s.output.fell_back = fell_back;
                    s.output.error = None;
                });
                self.output = Some(self.make_output(stream, writer, rate));
                true
            }
            Err(e) => {
                let message = e.to_string();
                self.status.update(|s| {
                    s.output.device_name = None;
                    s.output.fell_back = false;
                    s.output.error = Some(message);
                });
                false
            }
        }
    }

    fn make_output(&self, stream: Box<dyn OutputStream>, writer: FeedWriter, capture_rate: Option<u32>) -> Output {
        let device_rate = stream.sample_rate();
        let resampler = capture_rate
            .filter(|&rate| rate != device_rate)
            .map(|rate| LinearResampler::new(rate, device_rate));
        Output {
            stream,
            writer,
            capture_rate,
            resampler,
            draining: false,
        }
    }

    fn capture_rate(&self) -> Option<u32> {
        lock(&self.core).recorder.as_ref().map(|r| r.buffer().sample_rate())
    }

    /// Renders until the output has [`AHEAD`] queued, and closes it once a finished replay (or the beep) played.
    fn fill(&mut self) {
        let Some(output) = &self.output else { return };
        if let Some(failure) = output.stream.failure() {
            self.reopen(&failure);
        }
        let Some(output) = &mut self.output else { return };
        if output.draining {
            if output.writer.queued() == 0 {
                self.output = None;
                if self.ducked || self.status.get().playback != PlaybackState::Idle {
                    self.set_idle();
                }
            }
            return;
        }
        let device_rate = output.stream.sample_rate();
        let ahead = (AHEAD.as_secs_f64() * f64::from(device_rate)) as usize;
        while output.writer.queued() < ahead {
            let mut core = lock(&self.core);
            Self::sync_player(&mut self.player, &core);
            let (Some((player, _)), Some(recorder)) = (&mut self.player, &mut core.recorder) else {
                output.draining = true;
                break;
            };
            let rate = recorder.buffer().sample_rate();
            self.block.resize((BLOCK.as_secs_f64() * f64::from(rate)) as usize, 0.0);
            let rendered = player.render(recorder.buffer(), &mut self.block);
            drop(core);

            let block = &self.block[..rendered.written];
            let samples = match &mut output.resampler {
                Some(resampler) => {
                    self.resampled.clear();
                    resampler.process(block, &mut self.resampled);
                    &self.resampled[..]
                }
                None => block,
            };
            output.writer.write(samples);
            if rendered.ended {
                output.draining = true;
                break;
            }
        }
    }

    /// The stream failed mid-replay (an unplugged device): go on from where the player is, on the default device.
    fn reopen(&mut self, failure: &str) {
        let Some(old) = self.output.take() else { return };
        let draining = old.draining;
        drop(old);
        let (writer, feed) = OutputFeed::new(QUEUE_LEN);
        let fell_back = self.config.output_device.is_some();
        match self.backend.open(None, feed) {
            Ok(stream) => {
                let name = stream.device_name().to_owned();
                self.status.update(|s| {
                    s.output.device_name = Some(name);
                    s.output.fell_back = fell_back;
                    s.output.error = None;
                });
                let rate = self.capture_rate();
                let mut output = self.make_output(stream, writer, rate);
                output.draining = draining;
                self.output = Some(output);
            }
            Err(e) => {
                let message = format!("{failure}; {e}");
                self.status.update(|s| {
                    s.output.device_name = None;
                    s.output.error = Some(message);
                });
                self.stop();
            }
        }
    }

    /// Updates the playback state and how far behind live the replay is. `periodic` sends it even if unchanged.
    fn report_behind(&mut self, periodic: bool) {
        let behind = {
            let core = lock(&self.core);
            match (&self.player, &core.recorder) {
                (Some((player, _)), Some(recorder)) => player.behind(recorder.buffer()),
                _ => None,
            }
        };
        let playback = match (self.is_playing(), self.catching_up) {
            (false, _) => PlaybackState::Idle,
            (true, false) => PlaybackState::Replaying,
            (true, true) => PlaybackState::CatchingUp,
        };
        let changed = self.status.update(|s| {
            s.playback = playback;
            s.behind = behind;
        });
        if periodic && !changed {
            self.status.broadcast();
        }
        self.last_status = Instant::now();
    }
}
