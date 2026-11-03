//! AGC (Automatic Gain Control) for the hi-res microphone.
//!
//! Raises quiet speech toward a target level without pumping up background noise:
//! time constants are expressed in seconds and converted for the sample rate, and
//! the gain is held (not raised) while the input sits below a noise gate.

pub struct Agc {
    envelope: f32,
    gain: f32,
    attack: f32,
    release: f32,
    smoothing: f32,
}

impl Agc {
    const TARGET: f32 = 0.25; // ≈ -12 dBFS
    const MAX_GAIN: f32 = 8.0; // +18 dB
    /// Envelope level (≈ -50 dBFS) below which input is treated as background.
    const GATE: f32 = 0.003;
    const ATTACK_SECS: f32 = 0.005;
    const RELEASE_SECS: f32 = 0.4;
    const GAIN_SMOOTHING_SECS: f32 = 0.05;

    pub fn new(sample_rate: u32) -> Self {
        let coeff = |secs: f32| 1.0 - (-1.0 / (secs * sample_rate as f32)).exp();
        Self {
            envelope: 0.0,
            gain: 1.0,
            attack: coeff(Self::ATTACK_SECS),
            release: coeff(Self::RELEASE_SECS),
            smoothing: coeff(Self::GAIN_SMOOTHING_SECS),
        }
    }

    pub fn process(&mut self, samples: &mut [i16]) {
        for s in samples {
            let x = *s as f32 / 32768.0;
            let level = x.abs();

            let rate = if level > self.envelope {
                self.attack
            } else {
                self.release
            };
            self.envelope += (level - self.envelope) * rate;

            // Below the gate the input is background noise: hold the current gain
            // instead of raising it toward MAX_GAIN.
            let desired = if self.envelope < Self::GATE {
                self.gain
            } else {
                (Self::TARGET / self.envelope).min(Self::MAX_GAIN)
            };
            self.gain += (desired - self.gain) * self.smoothing;

            // Soft limiter
            let y = (x * self.gain).tanh();
            *s = (y * 32767.0) as i16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 64_000;

    fn sine(amplitude: f32, secs: f32) -> Vec<i16> {
        let n = (secs * RATE as f32) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                (amplitude * (2.0 * std::f32::consts::PI * 300.0 * t).sin() * 32767.0) as i16
            })
            .collect()
    }

    /// Deterministic noise with peak amplitude `amplitude`.
    fn noise(amplitude: f32, secs: f32) -> Vec<i16> {
        let mut state: u32 = 0x1234_5678;
        let n = (secs * RATE as f32) as usize;
        (0..n)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let unit = (state >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
                (unit * amplitude * 32767.0) as i16
            })
            .collect()
    }

    fn peak(samples: &[i16]) -> f32 {
        samples.iter().map(|&s| (s as f32 / 32768.0).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn background_below_gate_is_not_amplified() {
        let mut agc = Agc::new(RATE);
        let input = noise(0.001, 2.0); // ≈ -60 dBFS
        let mut output = input.clone();
        agc.process(&mut output);
        assert!(peak(&output) <= peak(&input) * 1.05);
    }

    #[test]
    fn quiet_speech_is_raised_toward_target() {
        let mut agc = Agc::new(RATE);
        let mut signal = sine(0.03, 3.0); // ≈ -30 dBFS
        agc.process(&mut signal);
        let tail = &signal[signal.len() - RATE as usize / 2..];
        let level = peak(tail);
        assert!(level > 0.15 && level < 0.35, "tail peak {level}");
    }

    #[test]
    fn gain_is_held_through_a_pause() {
        let mut agc = Agc::new(RATE);
        let mut speech = sine(0.03, 3.0);
        agc.process(&mut speech);
        let gain_after_speech = agc.gain;
        let mut pause = noise(0.001, 1.0);
        agc.process(&mut pause);
        assert!(agc.gain <= gain_after_speech * 1.01);
    }

    #[test]
    fn loud_input_never_exceeds_full_scale() {
        let mut agc = Agc::new(RATE);
        let mut signal = sine(0.95, 1.0);
        agc.process(&mut signal);
        assert!(signal.iter().all(|&s| s > i16::MIN));
        assert!(agc.gain < 1.0);
    }
}
