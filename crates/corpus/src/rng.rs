//! A small seeded generator and the skewed samplers the population uses.
//!
//! Nothing here reads a clock, an address or an environment variable: a stream is a function of
//! its seed and its label, so the same seed gives the same population byte for byte.

/// SplitMix64: one 64-bit state, a full period, and good enough statistics for test data.
#[derive(Clone, Debug)]
pub struct Rng(u64);

/// FNV-1a over a label, to give each stream of a seed its own start.
fn label_hash(label: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in label.bytes() {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    /// The stream of `seed` named `label`. Streams of different labels do not share state, so
    /// generating one object type does not move the numbers of another.
    pub fn derive(seed: u64, label: &str) -> Self {
        let mut rng = Rng(seed ^ label_hash(label));
        rng.next_u64();
        rng
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `0..n`; `n` must not be zero.
    pub fn below(&mut self, n: u64) -> u64 {
        debug_assert!(n > 0);
        // The bias of the modulo is far below anything a test of distributions can see.
        self.next_u64() % n
    }

    /// A number in `lo..=hi`.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        lo + self.below((hi - lo) as u64 + 1) as i64
    }

    /// A number in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }

    /// A skewed number in `lo..=hi`: most draws near `lo`, a long tail towards `hi`.
    pub fn skewed(&mut self, lo: i64, hi: i64) -> i64 {
        let u = self.unit();
        lo + ((hi - lo) as f64 * u * u * u) as i64
    }

    /// Shuffle in place (Fisher-Yates).
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i as u64 + 1) as usize;
            items.swap(i, j);
        }
    }
}

/// A Zipf distribution over `n` ranks with exponent `s`: rank 0 is drawn most, and the weight of
/// rank `k` is proportional to `1 / (k + 1)^s`. Sampling is a binary search in the cumulative
/// table.
#[derive(Clone, Debug)]
pub struct Zipf {
    cdf: Vec<f64>,
}

impl Zipf {
    pub fn new(n: usize, s: f64) -> Self {
        let mut cdf = Vec::with_capacity(n);
        let mut total = 0.0;
        for k in 0..n {
            total += 1.0 / ((k + 1) as f64).powf(s);
            cdf.push(total);
        }
        for v in &mut cdf {
            *v /= total;
        }
        Zipf { cdf }
    }

    pub fn len(&self) -> usize {
        self.cdf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cdf.is_empty()
    }

    /// A rank in `0..n`.
    pub fn sample(&self, rng: &mut Rng) -> usize {
        let u = rng.unit();
        let at = self.cdf.partition_point(|&c| c < u);
        at.min(self.cdf.len().saturating_sub(1))
    }
}
