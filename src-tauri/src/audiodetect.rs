//! Cameras: hearing a wormhole activation in one EVE client's sound.
//!
//! The app records each client's own sound (clientaudio.rs, Windows "process loopback") and
//! feeds it here in short pieces. A loud moment far above the client's usual sound starts a
//! look: about 0.6 s of it is turned into a "sound print" (how loud each pitch band is) and
//! compared with the activation sound the pilot taught it once ("Learn"). Close enough = heard.
//!
//! Same idea as the mapper's in-browser version (resources/js/lib/cameras/detector.ts), but on
//! one client's sound only, so Discord and the other clients can't set it off. No libraries:
//! a small FFT is below. All the tuning numbers are the constants here.

/// Sound comes in at this rate (the capture asks Windows for 48 kHz, converted if needed).
pub const RATE: usize = 48_000;
/// One look every 50 ms.
pub const BLOCK: usize = RATE / 20;
/// Samples per FFT (a power of two, under one block).
pub const FFT_SIZE: usize = 2048;
/// Pitch bands in a print (low sounds finer than high ones: an activation is mostly rumble).
pub const BANDS: usize = 32;
/// How alike a sound must be to the learned one (0-1).
pub const MATCH: f32 = 0.82;
/// A loud moment counts when it is this many spreads above the usual…
const SPREADS: f32 = 5.0;
/// …and at least this loud (RMS, full scale = 1), so a quiet client never counts.
const MIN_RMS: f32 = 0.02;
/// How fast "the usual" adapts (per block).
const ADAPT: f32 = 0.03;
/// Blocks to listen before anything counts (1.2 s).
const WARMUP: u32 = 24;
/// After a loud moment, this many blocks before the next (3 s).
const COOLDOWN: u32 = 60;
/// Blocks a print is made from (0.6 s).
const COLLECT: usize = 12;
const LOW_HZ: f32 = 30.0;
const HIGH_HZ: f32 = 8000.0;

/// What a piece of sound came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Heard {
    /// The activation sound (how alike, 0-1).
    Activation(f32),
    /// Learning: this is the sound now.
    Learned(Vec<f32>),
    /// Loud, but not the activation sound (how alike).
    Other(f32),
}

/// "Far above the usual": a running mean and spread, quiet for a while after each spike.
#[derive(Debug, Default)]
pub struct Spike {
    mean: f32,
    variance: f32,
    seen: u32,
    quiet: u32,
}

impl Spike {
    pub fn push(&mut self, value: f32) -> bool {
        self.seen += 1;
        if self.quiet > 0 {
            self.quiet -= 1;
        }
        let limit = (self.mean + SPREADS * self.variance.sqrt().max(0.002)).max(MIN_RMS);
        if self.seen > WARMUP && self.quiet == 0 && value >= limit {
            self.quiet = COOLDOWN;
            return true;
        }
        // A spike doesn't teach the usual (or the next jump would sound ordinary).
        if value < limit || self.seen <= WARMUP {
            let rate = if self.seen <= WARMUP { 1.0 / self.seen as f32 } else { ADAPT };
            let delta = value - self.mean;
            self.mean += rate * delta;
            self.variance = (1.0 - rate) * (self.variance + rate * delta * delta);
        }
        false
    }
}

/// In-place radix-2 FFT (re, im of length a power of two).
pub fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * std::f32::consts::PI / len as f32;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (sin, cos) = (angle * k as f32).sin_cos();
                let a = start + k;
                let b = a + len / 2;
                let tr = re[b] * cos - im[b] * sin;
                let ti = re[b] * sin + im[b] * cos;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

/// A sound print of FFT_SIZE samples: loudness per band (dB above -100), minus the middle band,
/// so only what stands out from the steady hum counts.
pub fn print(samples: &[f32]) -> Vec<f32> {
    let n = FFT_SIZE.min(samples.len());
    let mut re = vec![0.0f32; FFT_SIZE];
    let mut im = vec![0.0f32; FFT_SIZE];
    for i in 0..n {
        // Hann window.
        let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n.max(2) - 1) as f32).cos();
        re[i] = samples[samples.len() - n + i] * w;
    }
    fft(&mut re, &mut im);
    let bin_hz = RATE as f32 / FFT_SIZE as f32;
    let mut sums = [0.0f32; BANDS];
    let mut counts = [0u32; BANDS];
    for bin in 1..FFT_SIZE / 2 {
        let hz = bin as f32 * bin_hz;
        if !(LOW_HZ..=HIGH_HZ).contains(&hz) {
            continue;
        }
        let band = (((hz / LOW_HZ).ln() / (HIGH_HZ / LOW_HZ).ln()) * BANDS as f32).floor().min((BANDS - 1) as f32) as usize;
        let magnitude = (re[bin] * re[bin] + im[bin] * im[bin]).sqrt() / FFT_SIZE as f32;
        let db = 20.0 * (magnitude + 1e-9).log10();
        sums[band] += (db + 100.0).max(0.0);
        counts[band] += 1;
    }
    let levels: Vec<f32> = (0..BANDS).map(|b| if counts[b] > 0 { sums[b] / counts[b] as f32 } else { 0.0 }).collect();
    let mut sorted = levels.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let floor = sorted[BANDS / 2];
    levels.into_iter().map(|v| (v - floor).max(0.0)).collect()
}

pub fn average(prints: &[Vec<f32>]) -> Vec<f32> {
    if prints.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0.0f32; prints[0].len()];
    for p in prints {
        for (o, v) in out.iter_mut().zip(p) {
            *o += v / prints.len() as f32;
        }
    }
    out
}

/// How alike two prints are (cosine, 0-1).
pub fn likeness(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut left, mut right) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        left += x * x;
        right += y * y;
    }
    if left > 0.0 && right > 0.0 {
        (dot / (left * right).sqrt()).max(0.0)
    } else {
        0.0
    }
}

/// One client's ear: feed it mono samples at RATE as they come.
#[derive(Debug, Default)]
pub struct Ear {
    pending: Vec<f32>,
    spike: Spike,
    collecting: Option<Vec<Vec<f32>>>,
    /// The activation sound to match (None until taught).
    pub learned: Option<Vec<f32>>,
    /// The next loud moment becomes the learned sound.
    pub learning: bool,
    /// The last block's loudness (RMS), for the meter.
    pub level: f32,
}

impl Ear {
    pub fn new(learned: Option<Vec<f32>>) -> Self {
        Self { learned, ..Self::default() }
    }

    pub fn feed(&mut self, samples: &[f32]) -> Vec<Heard> {
        self.pending.extend_from_slice(samples);
        let mut out = Vec::new();
        while self.pending.len() >= BLOCK {
            let block: Vec<f32> = self.pending.drain(..BLOCK).collect();
            if let Some(heard) = self.block(&block) {
                out.push(heard);
            }
        }
        out
    }

    fn block(&mut self, block: &[f32]) -> Option<Heard> {
        let rms = (block.iter().map(|s| s * s).sum::<f32>() / block.len() as f32).sqrt();
        self.level = rms;
        if self.spike.push(rms) && self.collecting.is_none() {
            self.collecting = Some(Vec::new());
        }
        let prints = self.collecting.as_mut()?;
        prints.push(print(block));
        if prints.len() < COLLECT {
            return None;
        }
        let sound = average(prints);
        self.collecting = None;
        if self.learning {
            self.learning = false;
            self.learned = Some(sound.clone());
            return Some(Heard::Learned(sound));
        }
        let alike = likeness(self.learned.as_deref()?, &sound);
        Some(if alike >= MATCH { Heard::Activation(alike) } else { Heard::Other(alike) })
    }
}

/// Interleaved f32 frames (any channel count) to mono.
pub fn to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    let channels = channels.max(1);
    interleaved.chunks_exact(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tone of `hz` at `loud` (peak), `blocks` long, plus a little hiss.
    fn tone(hz: f32, loud: f32, blocks: usize, seed: &mut u32) -> Vec<f32> {
        (0..blocks * BLOCK)
            .map(|i| {
                *seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let hiss = ((*seed >> 16) as f32 / 65_535.0 - 0.5) * 0.004;
                loud * (2.0 * std::f32::consts::PI * hz * i as f32 / RATE as f32).sin() + hiss
            })
            .collect()
    }

    #[test]
    fn fft_finds_a_tone() {
        let mut re: Vec<f32> = (0..64).map(|i| (2.0 * std::f32::consts::PI * 8.0 * i as f32 / 64.0).sin()).collect();
        let mut im = vec![0.0; 64];
        fft(&mut re, &mut im);
        let peak = (0..32).max_by(|&a, &b| (re[a].hypot(im[a])).partial_cmp(&re[b].hypot(im[b])).unwrap()).unwrap();
        assert_eq!(peak, 8);
    }

    #[test]
    fn learns_then_hears_the_same_sound_but_not_another() {
        let mut seed = 7;
        let mut ear = Ear::new(None);
        // Quiet background, then the pilot presses Learn and a 120 Hz rumble plays.
        assert!(ear.feed(&tone(1000.0, 0.005, 40, &mut seed)).is_empty());
        ear.learning = true;
        let learned = ear.feed(&tone(120.0, 0.5, 14, &mut seed));
        assert!(matches!(learned.as_slice(), [Heard::Learned(_)]), "{learned:?}");
        assert!(ear.learned.is_some());

        // After the quiet spell, the same rumble again: heard.
        ear.feed(&tone(1000.0, 0.005, 80, &mut seed));
        let again = ear.feed(&tone(120.0, 0.5, 14, &mut seed));
        assert!(matches!(again.as_slice(), [Heard::Activation(a)] if *a >= MATCH), "{again:?}");

        // A loud high whistle (comms, a lock-on beep): loud, but not it.
        ear.feed(&tone(1000.0, 0.005, 80, &mut seed));
        let other = ear.feed(&tone(4000.0, 0.5, 14, &mut seed));
        assert!(matches!(other.as_slice(), [Heard::Other(a)] if *a < MATCH), "{other:?}");
    }

    #[test]
    fn nothing_before_it_has_learned_the_room_or_while_quiet() {
        let mut seed = 3;
        let mut ear = Ear::new(Some(vec![1.0; BANDS]));
        // Loud straight away (still warming up): nothing.
        assert!(ear.feed(&tone(120.0, 0.5, 10, &mut seed)).is_empty());
        // Steady quiet sound: nothing.
        let mut quiet = Ear::new(Some(vec![1.0; BANDS]));
        assert!(quiet.feed(&tone(200.0, 0.01, 100, &mut seed)).is_empty());
    }

    #[test]
    fn stereo_to_mono() {
        assert_eq!(to_mono(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
    }
}
