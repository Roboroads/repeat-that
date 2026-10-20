//! Development tool: runs a WAV file through the recorder and writes what a replay would contain.
//!
//! ```sh
//! cargo run -p rt-core --example trim_wav -- input.wav trimmed.wav [speed]
//! ```
//!
//! Only use it on recordings you're allowed to keep: this writes audio to disk, which the app itself never does.

use std::env;
use std::time::Duration;

use rt_core::{AudioFormat, Player, PlayerConfig, Recorder, SegmenterConfig, SileroVad};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let [_, input, output, rest @ ..] = args.as_slice() else {
        eprintln!("usage: trim_wav <input.wav> <output.wav> [speed]");
        std::process::exit(2);
    };
    let speed: f32 = rest.first().map(|s| s.parse()).transpose()?.unwrap_or(1.0);

    let mut reader = hound::WavReader::open(input)?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|s| s as f32 * scale))
                .collect::<Result<_, _>>()?
        }
    };
    let format = AudioFormat {
        sample_rate: spec.sample_rate,
        channels: spec.channels,
    };

    let mut recorder = Recorder::new(
        format,
        Box::new(SileroVad::new()?),
        SegmenterConfig::default(),
        Duration::from_secs(3600),
    );
    // 10 ms chunks, like a capture callback.
    let chunk = (spec.sample_rate as usize / 100) * spec.channels as usize;
    for part in samples.chunks(chunk) {
        recorder.push(part);
    }
    recorder.flush();

    let buffer = recorder.buffer();
    let input_secs = samples.len() as f32 / (spec.sample_rate as f32 * spec.channels as f32);
    println!(
        "{input_secs:.1} s in, {} utterances, {:.1} s of speech",
        buffer.len(),
        buffer.speech_duration().as_secs_f32()
    );
    for back in (0..buffer.len()).rev() {
        let u = buffer.get_back(back).unwrap();
        println!(
            "  #{:<3} {:5.2} s",
            u.id,
            u.samples.len() as f32 / spec.sample_rate as f32
        );
    }

    let mut player = Player::new(
        spec.sample_rate,
        PlayerConfig {
            speed,
            ..Default::default()
        },
    );
    // Step all the way back, then play through to the end.
    for _ in 0..buffer.len() {
        player.replay(buffer);
    }
    let mut out = Vec::new();
    let mut block = vec![0.0; 4096];
    loop {
        let rendered = player.render(buffer, &mut block);
        out.extend_from_slice(&block[..rendered.written]);
        if rendered.ended {
            break;
        }
    }

    let mut writer = hound::WavWriter::create(
        output,
        hound::WavSpec {
            channels: 1,
            sample_rate: spec.sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )?;
    for s in &out {
        writer.write_sample(*s)?;
    }
    writer.finalize()?;
    println!(
        "wrote {:.1} s at {speed}x to {output}",
        out.len() as f32 / spec.sample_rate as f32
    );
    Ok(())
}
