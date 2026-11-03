//! AGC (Automatic Gain Control) for the hi-res microphone.
//!
//! Raises quiet speech toward a target level without pumping up background noise:
//! time constants are expressed in seconds and converted for the sample rate, and
//! the gain is held (not raised) while the input sits below a noise gate.

use crate::utils::AgcTuning;

pub struct Agc {
    envelope: f32,
    gain: f32,
    target: f32,
    max_gain: f32,
    /// Envelope level below which input is treated as background.
    gate: f32,
    attack: f32,
    release: f32,
    smoothing: f32,
}

/// `value` limited to `[lo, hi]`; a non-finite value becomes `default`.
fn bounded(value: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(lo, hi)
    } else {
        default
    }
}

fn db_to_amplitude(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

impl Agc {
    const GAIN_SMOOTHING_SECS: f32 = 0.05;

    pub fn new(sample_rate: u32, tuning: &AgcTuning) -> Self {
        let defaults = AgcTuning::default();
        let coeff = |secs: f32| 1.0 - (-1.0 / (secs * sample_rate as f32)).exp();
        let target = bounded(tuning.target_dbfs, -30.0, -3.0, defaults.target_dbfs);
        let max_gain = bounded(tuning.max_gain_db, 0.0, 30.0, defaults.max_gain_db);
        let gate = bounded(tuning.gate_dbfs, -80.0, -20.0, defaults.gate_dbfs);
        let attack_ms = bounded(tuning.attack_ms, 1.0, 100.0, defaults.attack_ms);
        let release_ms = bounded(tuning.release_ms, 50.0, 3000.0, defaults.release_ms);
        Self {
            envelope: 0.0,
            gain: 1.0,
            target: db_to_amplitude(target),
            max_gain: db_to_amplitude(max_gain),
            gate: db_to_amplitude(gate),
            attack: coeff(attack_ms / 1000.0),
            release: coeff(release_ms / 1000.0),
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
            let desired = if self.envelope < self.gate {
                self.gain
            } else {
                (self.target / self.envelope).min(self.max_gain)
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

    #[test]
    fn tuning_is_bounded_and_non_finite_values_fall_back() {
        let wild = AgcTuning {
            target_dbfs: 10.0,
            max_gain_db: f32::NAN,
            gate_dbfs: -200.0,
            attack_ms: 0.0,
            release_ms: f32::INFINITY,
        };
        let agc = Agc::new(RATE, &wild);
        assert!((agc.target - db_to_amplitude(-3.0)).abs() < 1e-6);
        assert!((agc.max_gain - db_to_amplitude(18.0)).abs() < 1e-6);
        assert!((agc.gate - db_to_amplitude(-80.0)).abs() < 1e-9);
        assert!(agc.attack.is_finite() && agc.release.is_finite());
    }

    #[test]
    fn higher_target_gives_louder_speech() {
        let quiet = AgcTuning { target_dbfs: -20.0, ..AgcTuning::default() };
        let loud = AgcTuning { target_dbfs: -9.0, ..AgcTuning::default() };
        let mut a = sine(0.03, 3.0);
        let mut b = a.clone();
        Agc::new(RATE, &quiet).process(&mut a);
        Agc::new(RATE, &loud).process(&mut b);
        let tail = |s: &[i16]| peak(&s[s.len() - RATE as usize / 2..]);
        assert!(tail(&b) > tail(&a) * 1.5);
    }

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
        let mut agc = Agc::new(RATE, &AgcTuning::default());
        let input = noise(0.001, 2.0); // ≈ -60 dBFS
        let mut output = input.clone();
        agc.process(&mut output);
        assert!(peak(&output) <= peak(&input) * 1.05);
    }

    #[test]
    fn quiet_speech_is_raised_toward_target() {
        let mut agc = Agc::new(RATE, &AgcTuning::default());
        let mut signal = sine(0.03, 3.0); // ≈ -30 dBFS
        agc.process(&mut signal);
        let tail = &signal[signal.len() - RATE as usize / 2..];
        let level = peak(tail);
        assert!(level > 0.15 && level < 0.35, "tail peak {level}");
    }

    #[test]
    fn gain_is_held_through_a_pause() {
        let mut agc = Agc::new(RATE, &AgcTuning::default());
        let mut speech = sine(0.03, 3.0);
        agc.process(&mut speech);
        let gain_after_speech = agc.gain;
        let mut pause = noise(0.001, 1.0);
        agc.process(&mut pause);
        assert!(agc.gain <= gain_after_speech * 1.01);
    }

    #[test]
    fn loud_input_never_exceeds_full_scale() {
        let mut agc = Agc::new(RATE, &AgcTuning::default());
        let mut signal = sine(0.95, 1.0);
        agc.process(&mut signal);
        assert!(signal.iter().all(|&s| s > i16::MIN));
        assert!(agc.gain < 1.0);
    }
}
