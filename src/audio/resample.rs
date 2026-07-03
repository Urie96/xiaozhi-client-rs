/// Simple linear-interpolation resampler for mono i16 that handles
/// arbitrary input chunk sizes. Adequate for speech (8/16/24 kHz).
pub struct LinearResampler {
    from_rate: u32,
    to_rate: u32,
    last_sample: f32,
    /// Fractional read position in the input stream (units of input samples).
    pos: f64,
}

impl LinearResampler {
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        Self {
            from_rate,
            to_rate,
            last_sample: 0.0,
            pos: 0.0,
        }
    }

    pub fn ratio(&self) -> f64 {
        self.to_rate as f64 / self.from_rate as f64
    }

    /// Feed input i16 samples; returns resampled i16 output.
    /// `input` should be mono; caller is responsible for downmixing.
    pub fn process(&mut self, input: &[i16]) -> Vec<i16> {
        if self.from_rate == self.to_rate {
            return input.to_vec();
        }
        let step = self.ratio();
        // Each output sample corresponds to `1/step` input samples.
        // We track `pos` over the *extended* input (previous last + current).
        // For simplicity: build a small float buffer [last, ...input].
        let mut src: Vec<f32> = Vec::with_capacity(input.len() + 1);
        src.push(self.last_sample);
        for &s in input {
            src.push(s as f32 / 32768.0);
        }
        let n_in = src.len() as f64; // includes prev last at index 0
        // The base offset: input chunk starts at input index 0 in `src` is the
        // previous last sample (index 0). Real new samples are 1..n_in.
        // `pos` is the fractional index (in input samples) of the next output,
        // measured from the start of `src`.
        let mut out = Vec::new();
        while self.pos < n_in - 1.0 {
            let i0 = self.pos.floor() as usize;
            let frac = (self.pos - i0 as f64) as f32;
            let a = src[i0];
            let b = src[i0 + 1];
            let v = a + (b - a) * frac;
            out.push((v * 32768.0).clamp(-32768.0, 32767.0) as i16);
            self.pos += 1.0 / step;
        }
        // Carry over: subtract the consumed base so pos is relative to the
        // next chunk's src[0] (which will be the new last sample).
        let consumed = self.pos.floor();
        self.pos -= consumed;
        // The new "last sample" is the last input sample of this chunk.
        self.last_sample = *src.last().unwrap_or(&0.0);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_equal() {
        let mut r = LinearResampler::new(16000, 16000);
        let out = r.process(&[1, 2, 3]);
        assert_eq!(out, vec![1, 2, 3]);
    }

    #[test]
    fn downsample_half() {
        // 2x -> 1x: roughly half the samples
        let mut r = LinearResampler::new(2, 1);
        let input: Vec<i16> = (0..200).map(|i| i * 100).collect();
        let out = r.process(&input);
        assert!(out.len() > 80 && out.len() < 120, "len={}", out.len());
    }
}
