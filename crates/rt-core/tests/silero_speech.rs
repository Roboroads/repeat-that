//! End-to-end check with real speech: silence inserted around a recording is trimmed, the speech is kept.
#![cfg(feature = "silero")]

use std::time::Duration;

use rt_core::{AudioFormat, Player, PlayerConfig, Recorder, SegmenterConfig, SileroVad};

const RATE: u32 = 16_000;

fn load_fixture() -> Vec<f32> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/speech_8s.wav");
    let mut reader = hound::WavReader::open(path).expect("fixture exists");
    assert_eq!(reader.spec().sample_rate, RATE);
    reader.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect()
}

fn silence(seconds: f32) -> Vec<f32> {
    vec![0.0; (seconds * RATE as f32) as usize]
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

    let mut recorder = Recorder::new(
        AudioFormat {
            sample_rate: RATE,
            channels: 1,
        },
        Box::new(SileroVad::new().unwrap()),
        SegmenterConfig::default(),
        Duration::from_secs(60),
    );
    for chunk in input.chunks(160) {
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
    let played = player.render(buffer, &mut out);
    assert!(played > 0 && played < out.len());
}
