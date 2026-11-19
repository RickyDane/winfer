//! Sample-rate conversion and PCM16 packing for the realtime audio stream.

/// A streaming linear resampler: chunks in at one rate, chunks out at another, with no clicks
/// at the seams.
pub struct Resampler {
    ratio: f64,
    /// Position of the next output sample, in input samples from the start of the next chunk.
    pos: f64,
    last: f32,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self { ratio: from as f64 / to.max(1) as f64, pos: 0.0, last: 0.0 }
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::with_capacity((input.len() as f64 / self.ratio) as usize + 2);
        // Index -1 is the last sample of the previous chunk.
        let at = |i: i64, last: f32| if i < 0 { last } else { input[i as usize] };
        while self.pos < input.len() as f64 - 1.0 + 1e-9 && (self.pos.floor() as usize) < input.len() {
            let i = self.pos.floor() as i64;
            let frac = (self.pos - i as f64) as f32;
            let a = at(i, self.last);
            let b = if (i + 1) as usize >= input.len() { a } else { input[(i + 1) as usize] };
            out.push(a + (b - a) * frac);
            self.pos += self.ratio;
        }
        self.pos -= input.len() as f64;
        self.last = input[input.len() - 1];
        // `pos` can sit before the next chunk's first sample, which interpolates from `last`.
        out
    }
}

/// Mono from interleaved channels.
pub fn downmix(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    samples.chunks_exact(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect()
}

pub fn to_pcm16(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    out
}

pub fn from_pcm16(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect()
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_round_trips() {
        let samples = [0.0, 0.5, -0.5, 1.0, -1.0];
        let back = from_pcm16(&to_pcm16(&samples));
        for (a, b) in samples.iter().zip(&back) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
        assert_eq!(to_pcm16(&[2.0]), 32767i16.to_le_bytes());
    }

    #[test]
    fn downmix_averages_channels() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), [0.5, 0.5]);
        assert_eq!(downmix(&[0.25, 0.75], 1), [0.25, 0.75]);
    }

    #[test]
    fn resampling_keeps_the_length_ratio_across_chunks() {
        let mut r = Resampler::new(48_000, 24_000);
        let ramp: Vec<f32> = (0..4800).map(|i| i as f32 / 4800.0).collect();
        let mut out = Vec::new();
        for chunk in ramp.chunks(480) {
            out.extend(r.process(chunk));
        }
        assert!((out.len() as i64 - 2400).abs() <= 2, "{}", out.len());
        // A rising ramp stays rising: no seams.
        assert!(out.windows(2).all(|w| w[1] >= w[0] - 1e-6));
    }

    #[test]
    fn upsampling_interpolates() {
        let mut r = Resampler::new(24_000, 48_000);
        let out = r.process(&[0.0, 1.0, 0.0, 1.0]);
        assert!(out.len() >= 7 && out.len() <= 8);
        assert!((out[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn rms_of_a_square_wave() {
        assert!((rms(&[0.5, -0.5, 0.5, -0.5]) - 0.5).abs() < 1e-6);
        assert_eq!(rms(&[]), 0.0);
    }
}
