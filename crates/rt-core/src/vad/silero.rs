//! [Silero VAD](https://github.com/snakers4/silero-vad) (MIT license, see `models/LICENSE-silero-vad`) running on
//! ONNX Runtime.

use std::sync::OnceLock;

use ort::environment::Environment;
use ort::session::Session;
use ort::value::Tensor;

use super::{VAD_SAMPLE_RATE, Vad};

static MODEL: &[u8] = include_bytes!("../../models/silero_vad.onnx");

/// Samples per frame the model is trained on at 16 kHz.
const FRAME_LEN: usize = 512;
/// The model expects the tail of the previous frame in front of each new one.
const CONTEXT_LEN: usize = 64;
/// Recurrent state shape: `[2, batch, 128]`.
const STATE_LEN: usize = 2 * 128;

pub type Error = ort::Error;

pub struct SileroVad {
    session: Session,
    state: Vec<f32>,
    context: [f32; CONTEXT_LEN],
    input: Vec<f32>,
}

impl SileroVad {
    pub fn new() -> Result<Self, Error> {
        let session = Session::builder(environment()?)?
            .with_intra_threads(1)?
            .with_inter_threads(1)?
            .commit_from_memory(MODEL)?;
        Ok(Self {
            session,
            state: vec![0.0; STATE_LEN],
            context: [0.0; CONTEXT_LEN],
            input: Vec::with_capacity(CONTEXT_LEN + FRAME_LEN),
        })
    }

    fn infer(&mut self, frame: &[f32]) -> Result<f32, Error> {
        self.input.clear();
        self.input.extend_from_slice(&self.context);
        self.input.extend_from_slice(frame);
        self.context.copy_from_slice(&frame[frame.len() - CONTEXT_LEN..]);

        let input = Tensor::from_array(([1, self.input.len()], self.input.clone()))?;
        let state = Tensor::from_array(([2, 1, 128], self.state.clone()))?;
        let sr = Tensor::from_array(((), vec![VAD_SAMPLE_RATE as i64]))?;
        let outputs = self.session.run(ort::inputs![
            "input" => input,
            "state" => state,
            "sr" => sr,
        ])?;

        let (_, probability) = outputs["output"].try_extract_tensor::<f32>()?;
        let (_, new_state) = outputs["stateN"].try_extract_tensor::<f32>()?;
        let probability = probability[0];
        self.state.copy_from_slice(new_state);
        Ok(probability)
    }
}

fn environment() -> Result<&'static Environment, Error> {
    static ENV: OnceLock<Environment> = OnceLock::new();
    if let Some(env) = ENV.get() {
        return Ok(env);
    }
    let env = ort::init().with_name("repeat-that").with_telemetry(false).build()?;
    Ok(ENV.get_or_init(|| env))
}

impl Vad for SileroVad {
    fn frame_len(&self) -> usize {
        FRAME_LEN
    }

    fn speech_probability(&mut self, frame: &[f32]) -> f32 {
        debug_assert_eq!(frame.len(), FRAME_LEN);
        // A failed inference is treated as silence rather than taking down the capture thread.
        self.infer(frame).unwrap_or(0.0)
    }

    fn reset(&mut self) {
        self.state.fill(0.0);
        self.context.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_not_speech() {
        let mut vad = SileroVad::new().expect("model loads");
        for _ in 0..20 {
            assert!(vad.speech_probability(&[0.0; FRAME_LEN]) < 0.1);
        }
    }

    #[test]
    fn steady_tone_is_not_speech() {
        let mut vad = SileroVad::new().expect("model loads");
        let tone: Vec<f32> = (0..FRAME_LEN * 30)
            .map(|i| 0.3 * (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / VAD_SAMPLE_RATE as f32).sin())
            .collect();
        let last = tone
            .as_chunks::<FRAME_LEN>()
            .0
            .iter()
            .map(|f| vad.speech_probability(f))
            .last()
            .unwrap();
        assert!(last < 0.5, "probability {last}");
    }
}
