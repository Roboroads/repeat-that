//! Capturing one app: a capture stream of our own, linked by hand to the output ports of the app's playback streams.
//!
//! The stream never connects on its own (no `AUTOCONNECT`, `node.autoconnect = false`), so the session manager links
//! nothing to it, and only ports of `Stream/Output/Audio` nodes are ever linked: a microphone or any other source can't
//! end up in the capture. Its links are passive, so capturing never keeps an app's stream running.
//!
//! Playback streams apply their volume before their output ports, so a ducked app is captured ducked. The capture
//! undoes that: each cycle's audio is amplified by the inverse of the level the app played it at, from the
//! [`DuckLevel`] history. A volume change takes effect at a cycle boundary of the graph, so one level per cycle is
//! exact; see `ramp.rs` for how well the estimate of that boundary holds. The capture gets the app's streams mixed, so
//! it undoes the duck only while all of them play ducked (see [`Compensation`]).

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use pipewire as pw;
use pw::core::CoreRc;
use pw::properties::PropertiesBox;
use pw::spa;
use pw::stream::{StreamFlags, StreamListener, StreamRc, StreamState};
use spa::param::ParamType;
use spa::param::audio::{AudioFormat, AudioInfoRaw};
use spa::pod::serialize::PodSerializer;
use spa::pod::{Object, Pod, Value};
use spa::utils::{Direction, SpaTypes};

use crate::{CaptureFormat, DUCK_HISTORY_MAX, DuckChange, DuckLevel, FrameSink};

/// The format we capture in. The rate is the graph's, which the stream reports once it is negotiated.
const CHANNELS: u16 = 2;

/// The class of the only nodes we ever link from: playback streams of apps.
pub(super) const PLAYBACK: &str = "Stream/Output/Audio";

/// The most frames a cycle can have (PipeWire's quantum limit), so the capture thread's buffer never grows.
const MAX_FRAMES: usize = 8192;

/// Where a duck level change is assumed to reach the audio, after the call that makes it, in 100 ns units. The call
/// goes to the app's stream through the daemon, which takes well under a millisecond on an idle system, and the
/// stream applies it from its next cycle on.
pub(super) const VOLUME_DELAY: u64 = 10_000;

/// A port, as the registry announces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PortInfo {
    pub node: u32,
    pub output: bool,
    /// `audio.channel`, e.g. `FL`, `MONO` or `AUX3`.
    pub channel: String,
    /// Whether it carries audio (`format.dsp`), rather than MIDI or control data.
    pub audio: bool,
    /// Whether it is a monitor port, which carries what goes into a sink.
    pub monitor: bool,
}

/// Which of our two input ports, left (0) and right (1), an output port with channel position `channel` feeds.
fn inputs_for(channel: &str) -> &'static [usize] {
    const LEFT: &[&str] = &[
        "FL", "SL", "RL", "FLC", "RLC", "FLW", "FLH", "TFL", "TRL", "TSL", "TFLC", "BLC",
    ];
    const RIGHT: &[&str] = &[
        "FR", "SR", "RR", "FRC", "RRC", "FRW", "FRH", "TFR", "TRR", "TSR", "TFRC", "BRC",
    ];
    const SILENT: &[&str] = &["LFE", "LFE2", "LLFE", "RLFE"];
    if let Some(aux) = channel.strip_prefix("AUX").and_then(|n| n.parse::<u32>().ok()) {
        return if aux % 2 == 0 { &[0] } else { &[1] };
    }
    if LEFT.contains(&channel) {
        &[0]
    } else if RIGHT.contains(&channel) {
        &[1]
    } else if SILENT.contains(&channel) {
        &[]
    } else {
        // Mono, centre channels and anything unknown go to both sides.
        &[0, 1]
    }
}

/// The links (output port, input port) that capture the nodes in `app` into our capture node `capture`. Only output
/// ports of nodes whose class (`class_of`) is [`PLAYBACK`] are linked, whatever `app` holds: that is what keeps a
/// microphone out.
pub(super) fn plan_links<'a>(
    ports: &HashMap<u32, PortInfo>,
    app: &BTreeSet<u32>,
    class_of: impl Fn(u32) -> Option<&'a str>,
    capture: u32,
) -> BTreeSet<(u32, u32)> {
    let mut inputs: [Option<u32>; 2] = [None, None];
    for (&id, port) in ports {
        if port.node == capture && !port.output && port.audio {
            match port.channel.as_str() {
                "FL" => inputs[0] = Some(id),
                "FR" => inputs[1] = Some(id),
                _ => {}
            }
        }
    }
    let mut links = BTreeSet::new();
    for (&id, port) in ports {
        let playback = class_of(port.node) == Some(PLAYBACK);
        if !port.output || !port.audio || port.monitor || !playback || !app.contains(&port.node) {
            continue;
        }
        for &side in inputs_for(&port.channel) {
            if let Some(input) = inputs[side] {
                links.insert((id, input));
            }
        }
    }
    links
}

/// How a capture undoes its app's duck: the app's duck level, whether the app's streams are asynchronous (`node.async`),
/// and whether to undo it at all. That is only right while every stream of the app plays ducked: one the duck left
/// alone, because the user set its volume during the duck or its volume wasn't known yet, plays at its own volume, and
/// undoing the duck on the mix would boost it by up to 20 times. Then the capture takes the app as it plays.
#[derive(Clone)]
pub(super) struct Compensation {
    pub level: DuckLevel,
    pub asynchronous: Arc<AtomicBool>,
    pub undo: Arc<AtomicBool>,
}

impl Compensation {
    pub fn new(level: DuckLevel) -> Self {
        Self {
            level,
            asynchronous: Arc::default(),
            undo: Arc::new(AtomicBool::new(true)),
        }
    }
}

/// The duck level audio played at `time` (100 ns units) was played at, from `history` (oldest first).
pub(super) fn level_at(history: &[DuckChange], time: u64) -> f32 {
    history
        .iter()
        .rev()
        .find(|c| c.since <= time)
        .or(history.first())
        .map_or(1.0, |c| c.level)
}

/// Undoes a duck to `level` on `samples`.
fn amplify(samples: &mut [f32], level: f32) {
    if level < 1.0 {
        let gain = 1.0 / level;
        samples.iter_mut().for_each(|s| *s *= gain);
    }
}

/// The current time on the clock of the graph's cycle times (`CLOCK_MONOTONIC`), in 100 ns units.
pub(super) fn now() -> u64 {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) };
    (time.tv_sec as u64) * 10_000_000 + (time.tv_nsec as u64) / 100
}

/// Our capture stream, and its callbacks. Dropping it disconnects the stream first, so the real-time thread is done
/// with the callbacks before they go.
pub(super) struct CaptureStream {
    // Field order is drop order, after `Drop::drop` disconnected the stream.
    _process: StreamListener<()>,
    _events: StreamListener<()>,
    stream: StreamRc,
}

impl Drop for CaptureStream {
    fn drop(&mut self) {
        let _ = self.stream.disconnect();
    }
}

/// Opens a capture stream named `node_name` that hands what it captures to `sink`, with the app's duck undone as
/// `compensation` says. `running` turns false when the stream fails. Each cycle stores its length in `quantum`
/// (nanoseconds), which the fades use as their step.
pub(super) fn open(
    core: &CoreRc,
    node_name: &str,
    mut sink: FrameSink,
    compensation: Compensation,
    running: Arc<AtomicBool>,
    quantum: Arc<AtomicU64>,
) -> Result<CaptureStream, pw::Error> {
    let Compensation {
        level,
        asynchronous,
        undo,
    } = compensation;
    let mut props = PropertiesBox::new();
    props.insert("media.type", "Audio");
    props.insert("media.category", "Capture");
    props.insert("media.class", "Stream/Input/Audio");
    props.insert("node.name", node_name);
    props.insert("node.description", "Repeat That capture");
    props.insert("node.autoconnect", "false");
    props.insert("node.dont-reconnect", "true");
    props.insert("node.passive", "true");
    props.insert("audio.position", "[ FL FR ]");
    let stream = StreamRc::new(core.clone(), "repeat-that-capture", props)?;

    // The graph rate, once negotiated. Shared between the two callbacks, which run on different threads.
    let rate = Arc::new(AtomicU32::new(0));
    let negotiated = Arc::clone(&rate);
    let failed = Arc::clone(&running);
    let events = stream
        .add_local_listener_with_user_data(())
        .param_changed(move |_, _, id, param| {
            if id != ParamType::Format.as_raw() {
                return;
            }
            let Some(param) = param else { return };
            let mut info = AudioInfoRaw::new();
            if info.parse(param).is_ok() {
                negotiated.store(info.rate(), Ordering::Relaxed);
            }
        })
        .state_changed(move |_, _, _, state| {
            if matches!(state, StreamState::Error(_) | StreamState::Unconnected) {
                failed.store(false, Ordering::Relaxed);
            }
        })
        .register()?;

    // Runs on PipeWire's real-time thread: no locks it waits for, no allocations.
    let mut samples: Vec<f32> = Vec::with_capacity(MAX_FRAMES * usize::from(CHANNELS));
    let mut history: Vec<DuckChange> = Vec::with_capacity(DUCK_HISTORY_MAX);
    let process = stream
        .add_local_listener_with_user_data(())
        .process(move |stream, _| {
            let rate = rate.load(Ordering::Relaxed);
            // The cycle this audio belongs to started at `now`.
            let cycle = stream.time().map(|t| t.now()).unwrap_or(0).max(0) as u64 / 100;
            // While a fade changes the history, the copy from the cycle before does: it is a cycle old at most.
            if !level.try_history(&mut history) && history.is_empty() {
                history.push(DuckChange { since: 0, level: 1.0 });
            }
            while let Some(mut buffer) = stream.dequeue_buffer() {
                let Some(data) = buffer.datas_mut().first_mut() else {
                    continue;
                };
                let (offset, size) = (data.chunk().offset() as usize, data.chunk().size() as usize);
                let Some(bytes) = data.data() else { continue };
                let Some(bytes) = bytes.get(offset..offset + size) else {
                    continue;
                };
                samples.clear();
                let floats = bytes.as_chunks::<4>().0;
                let room = samples.capacity();
                samples.extend(floats.iter().take(room).map(|&b| f32::from_le_bytes(b)));
                if samples.is_empty() || rate == 0 {
                    continue;
                }
                let frames = samples.len() / usize::from(CHANNELS);
                let length = frames as u64 * 1_000_000_000 / u64::from(rate);
                quantum.store(length, Ordering::Relaxed);
                // An asynchronous stream's audio is a cycle old when we get it: it was made in the cycle before.
                let played = if asynchronous.load(Ordering::Relaxed) {
                    cycle.saturating_sub(length / 100)
                } else {
                    cycle
                };
                if undo.load(Ordering::Relaxed) {
                    amplify(&mut samples, level_at(&history, played));
                }
                sink(
                    &samples,
                    CaptureFormat {
                        sample_rate: rate,
                        channels: CHANNELS,
                    },
                );
            }
        })
        .register()?;

    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::F32LE);
    info.set_channels(u32::from(CHANNELS));
    let mut position = [0; spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    let format = Value::Object(Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    });
    let (format, _) =
        PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &format).map_err(|_| pw::Error::CreationFailed)?;
    let format = format.into_inner();
    let mut params = [Pod::from_bytes(&format).ok_or(pw::Error::CreationFailed)?];
    stream.connect(
        Direction::Input,
        None,
        StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS | StreamFlags::DONT_RECONNECT,
        &mut params,
    )?;
    Ok(CaptureStream {
        _process: process,
        _events: events,
        stream,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(node: u32, output: bool, channel: &str) -> PortInfo {
        PortInfo {
            node,
            output,
            channel: channel.into(),
            audio: true,
            monitor: false,
        }
    }

    /// Our capture node is 10, with inputs 100 (FL) and 101 (FR).
    fn with_capture(mut ports: HashMap<u32, PortInfo>) -> HashMap<u32, PortInfo> {
        ports.insert(100, port(10, false, "FL"));
        ports.insert(101, port(10, false, "FR"));
        ports
    }

    fn classes(id: u32) -> Option<&'static str> {
        match id {
            10 => Some("Stream/Input/Audio"),
            20..=22 => Some(PLAYBACK),
            30 => Some("Audio/Source"),
            31 => Some("Audio/Sink"),
            _ => None,
        }
    }

    #[test]
    fn stereo_links_side_by_side_and_mono_to_both() {
        let ports = with_capture(HashMap::from([
            (200, port(20, true, "FL")),
            (201, port(20, true, "FR")),
            (210, port(21, true, "MONO")),
        ]));
        let links = plan_links(&ports, &BTreeSet::from([20, 21]), classes, 10);
        assert_eq!(links, BTreeSet::from([(200, 100), (201, 101), (210, 100), (210, 101)]));
    }

    #[test]
    fn surround_folds_into_stereo() {
        let channels = ["FL", "FR", "FC", "LFE", "RL", "RR", "AUX0", "AUX1"];
        let ports = with_capture(
            channels
                .iter()
                .enumerate()
                .map(|(i, &c)| (300 + i as u32, port(22, true, c)))
                .collect(),
        );
        let links = plan_links(&ports, &BTreeSet::from([22]), classes, 10);
        let to = |input| {
            links
                .iter()
                .filter(|l| l.1 == input)
                .map(|l| l.0 - 300)
                .collect::<Vec<_>>()
        };
        // FL, FC, RL, AUX0 on the left; FR, FC, RR, AUX1 on the right; no LFE.
        assert_eq!(to(100), [0, 2, 4, 6]);
        assert_eq!(to(101), [1, 2, 5, 7]);
    }

    #[test]
    fn microphones_and_other_sources_are_never_linked() {
        // Even when the app's nodes would include a source, a sink's monitor or a MIDI port.
        let mut midi = port(20, true, "FL");
        midi.audio = false;
        let mut monitor = port(31, true, "FL");
        monitor.monitor = true;
        let ports = with_capture(HashMap::from([
            (300, port(30, true, "FL")),
            (301, port(30, true, "FR")),
            (310, monitor),
            (311, port(31, true, "FR")),
            (320, midi),
            (330, port(10, true, "FL")),
        ]));
        let links = plan_links(&ports, &BTreeSet::from([10, 20, 30, 31]), classes, 10);
        assert!(links.is_empty(), "{links:?}");
    }

    #[test]
    fn only_the_apps_nodes_are_linked() {
        let ports = with_capture(HashMap::from([
            (200, port(20, true, "FL")),
            (210, port(21, true, "FL")),
        ]));
        assert_eq!(
            plan_links(&ports, &BTreeSet::from([21]), classes, 10),
            BTreeSet::from([(210, 100)])
        );
        // Without our input ports yet, nothing is linked.
        let ports = HashMap::from([(200, port(20, true, "FL"))]);
        assert!(plan_links(&ports, &BTreeSet::from([20]), classes, 10).is_empty());
    }

    #[test]
    fn each_cycle_gets_the_level_it_was_played_at() {
        let history = [
            DuckChange { since: 0, level: 1.0 },
            DuckChange {
                since: 1_000,
                level: 0.5,
            },
            DuckChange {
                since: 2_000,
                level: 0.25,
            },
        ];
        assert_eq!(level_at(&history, 999), 1.0);
        assert_eq!(level_at(&history, 1_000), 0.5);
        assert_eq!(level_at(&history, 5_000), 0.25);
        // Older than the history: its oldest level, which covers everything a capture can still drain.
        let trimmed = &history[1..];
        assert_eq!(level_at(trimmed, 10), 0.5);
        assert_eq!(level_at(&[], 10), 1.0);

        let mut samples = vec![0.25f32; 4];
        amplify(&mut samples, level_at(&history, 5_000));
        assert_eq!(samples, [1.0; 4]);
        amplify(&mut samples, 1.0);
        assert_eq!(samples, [1.0; 4]);
    }

    #[test]
    fn the_clock_is_monotonic_in_100_ns_units() {
        let (a, b) = (now(), now());
        assert!(b >= a);
        std::thread::sleep(std::time::Duration::from_millis(5));
        let elapsed = now() - b;
        assert!((40_000..2_000_000).contains(&elapsed), "{elapsed}");
    }
}
