//! Seeded randomness: the generator the simulator uses, and a Zipf sampler over ranks.

use std::collections::BTreeMap;
use std::rc::Rc;

/// SplitMix64.
#[derive(Clone, Debug)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `bound` (0 when `bound` is 0).
    pub fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            0
        } else {
            self.next_u64() % bound
        }
    }

    /// A number in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A stable 64-bit hash of a text (FNV-1a), to derive a stream from a name.
pub fn hash_text(text: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.bytes() {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Ranks 0..n with weight 1/(rank+1)^s: rank 0 is the hottest. `s` = 0 is uniform.
pub struct Zipf {
    cdf: Vec<f64>,
}

impl Zipf {
    pub fn new(n: usize, s: f64) -> Zipf {
        let mut cdf = Vec::with_capacity(n);
        let mut total = 0.0;
        for rank in 0..n {
            total += ((rank + 1) as f64).powf(-s);
            cdf.push(total);
        }
        if total > 0.0 {
            for entry in &mut cdf {
                *entry /= total;
            }
        }
        Zipf { cdf }
    }

    pub fn len(&self) -> usize {
        self.cdf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cdf.is_empty()
    }

    /// A rank; 0 when the population is empty.
    pub fn sample(&self, rng: &mut Rng) -> usize {
        if self.cdf.is_empty() {
            return 0;
        }
        let u = rng.unit();
        self.cdf.partition_point(|&p| p < u).min(self.cdf.len() - 1)
    }
}

/// Zipf tables by (population, exponent), built once.
#[derive(Default)]
pub struct ZipfCache {
    tables: BTreeMap<(usize, u64), Rc<Zipf>>,
}

impl ZipfCache {
    pub fn get(&mut self, n: usize, s: f64) -> Rc<Zipf> {
        self.tables
            .entry((n, s.to_bits()))
            .or_insert_with(|| Rc::new(Zipf::new(n, s)))
            .clone()
    }
}
