//! End-to-end checks with real speech: silence inserted around a recording is trimmed and the speech kept, and a
//! replay started while someone talks plays through to live.
#![cfg(feature = "silero")]

use std::time::Duration;

use rt_core::{AudioFormat, Player, PlayerConfig, Recorder, SegmenterConfig, SileroVad, SpeechBuffer};

const RATE: u32 = 16_000;
/// 10 ms, like a capture callback.
const CHUNK: usize = RATE as usize / 100;

fn load_fixture() -> Vec<f32> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/speech_8s.wav");
    let mut reader = hound::WavReader::open(path).expect("fixture exists");
    assert_eq!(reader.spec().sample_rate, RATE);
    reader.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect()
}

fn silence(seconds: f32) -> Vec<f32> {
    vec![0.0; (seconds * RATE as f32) as usize]
}

fn recorder() -> Recorder {
    Recorder::new(
        AudioFormat {
            sample_rate: RATE,
            channels: 1,
        },
        Box::new(SileroVad::new().unwrap()),
        SegmenterConfig::default(),
        Duration::from_secs(60),
    )
}

fn energy(samples: &[f32]) -> f64 {
    samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum()
}

#[test]
fn trims_inserted_silence_from_real_speech() {
    let speech = load_fixture();
    let (first, second) = speech.split_at(speech.len() / 2);
    let input = [
        silence(3.0),
        first.to_vec(),
        silence(4.0),
        second.to_vec(),
        silence(3.0),
    ]
    .concat();

    let mut recorder = recorder();
    for chunk in input.chunks(CHUNK) {
        recorder.push(chunk);
    }
    recorder.flush();

    let buffer = recorder.buffer();
    let kept = buffer.speech_duration().as_secs_f32();
    // 18 s in, of which 8 s is the recording (which has short pauses of its own).
    assert!((5.0..9.5).contains(&kept), "kept {kept} s");
    // The inserted 4 s gap always ends an utterance.
    assert!(buffer.len() >= 2, "{} utterances", buffer.len());

    let mut player = Player::new(RATE, PlayerConfig::default());
    assert!(player.replay(buffer));
    let mut out = vec![0.0; RATE as usize * 20];
    let rendered = player.render(buffer, &mut out);
    assert!(rendered.ended);
    assert!(rendered.written > 0 && rendered.written < out.len());
}

/// What a replay started `start` into the fixture played, rendering `blocks_per_chunk` 10 ms blocks at 1.0x for every
/// 10 ms captured, until it ended.
struct PlayThrough {
    /// The recorder's buffer after the final flush.
    recorder: Recorder,
    first_id: u64,
    out: Vec<f32>,
    /// How close the replay got to live while it played along with the capture.
    min_behind: Duration,
    /// The replay ended on its own after the speaker stopped, before the capture ended.
    ended_before_flush: bool,
}

fn play_through(start: Duration, blocks_per_chunk: usize) -> PlayThrough {
    let speech = load_fixture();
    let mut recorder = recorder();
    let mut player = Player::new(RATE, PlayerConfig::default());
    let mut out = Vec::new();
    let mut block = [0.0; CHUNK];
    let mut render = |player: &mut Player, buffer: &SpeechBuffer| {
        let rendered = player.render(buffer, &mut block);
        out.extend_from_slice(&block[..rendered.written]);
        rendered.ended
    };

    let start_at = (start.as_secs_f64() * RATE as f64) as usize / CHUNK;
    let mut first_id = None;
    let mut min_behind = Duration::MAX;
    let mut ended = false;
    for (i, chunk) in speech.chunks(CHUNK).enumerate() {
        recorder.push(chunk);
        if i == start_at {
            assert!(player.replay(recorder.buffer()));
            first_id = player.current_id();
        }
        if first_id.is_none() || ended {
            continue;
        }
        for _ in 0..blocks_per_chunk {
            ended = render(&mut player, recorder.buffer());
            if ended {
                // Only once the speaker has stopped: their last sentence is finished and nothing new has started.
                let buffer = recorder.buffer();
                assert!(
                    buffer.open_id().is_none() && !buffer.speech_pending(),
                    "ended while still talking"
                );
                break;
            }
            min_behind = min_behind.min(player.behind(recorder.buffer()).unwrap());
        }
    }

    // The capture ends: the replay plays what's left, then ends.
    let ended_before_flush = ended;
    recorder.flush();
    for _ in 0..1000 {
        if ended {
            break;
        }
        ended = render(&mut player, recorder.buffer());
    }
    assert!(ended, "replay never ended");
    assert_eq!(player.behind(recorder.buffer()), None);
    PlayThrough {
        recorder,
        first_id: first_id.expect("replay started"),
        out,
        min_behind,
        ended_before_flush,
    }
}

/// At normal speed a replay is the utterances themselves with silence in between, so it holds exactly their energy,
/// from the first one played to the last.
fn assert_plays_every_utterance(result: &PlayThrough) {
    let buffer = result.recorder.buffer();
    let utterances: Vec<&[f32]> = (result.first_id..=buffer.latest_id().unwrap())
        .map(|id| buffer.get(id).unwrap().samples.as_slice())
        .collect();
    let expected = utterances.iter().map(|u| energy(u)).sum::<f64>();
    let played = energy(&result.out);
    assert!(
        (played - expected).abs() <= expected * 1e-6,
        "played {played}, expected {expected}"
    );
    let speech_len: usize = utterances.iter().map(|u| u.len()).sum();
    assert!(result.out.len() > speech_len);
}

#[test]
fn replay_during_speech_plays_through_to_live() {
    // Started 4 s into a sentence, the replay starts at its beginning and stays 4 s behind it, so it is still playing
    // when the capture ends.
    let result = play_through(Duration::from_secs(4), 1);
    assert_eq!(result.first_id, 0);
    assert!(result.min_behind > Duration::from_secs(3), "{:?}", result.min_behind);
    assert!(!result.ended_before_flush);
    assert_plays_every_utterance(&result);
}

#[test]
fn replay_follows_the_speaker_once_caught_up() {
    // Rendering ahead of real time catches up with the speaker, after which the replay follows them as they talk and
    // ends once they stop (the recording ends with about a second of silence).
    let result = play_through(Duration::from_secs(2), 2);
    assert!(
        result.min_behind < Duration::from_millis(200),
        "{:?}",
        result.min_behind
    );
    assert!(result.ended_before_flush);
    assert_plays_every_utterance(&result);
}
