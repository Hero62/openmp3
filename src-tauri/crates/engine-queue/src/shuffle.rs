//! Shuffle orderings.
//!
//! * `bag`: a uniform random permutation — every track plays once before any
//!   repeats (the "shuffle bag").
//! * `spread`: artist-balanced shuffle. Each artist's tracks are spaced evenly
//!   across the whole order with a random offset and jitter, then everything is
//!   sorted by that position (Fiedler's "balanced shuffle"). Same-artist runs
//!   become rare without becoming predictable.

/// Small, seedable RNG so tests are deterministic.
pub struct Rng(fastrand::Rng);

impl Rng {
    pub fn new() -> Self {
        Self(fastrand::Rng::new())
    }
    pub fn seeded(seed: u64) -> Self {
        Self(fastrand::Rng::with_seed(seed))
    }
    pub fn below(&mut self, n: usize) -> usize {
        self.0.usize(..n.max(1))
    }
    pub fn f64(&mut self) -> f64 {
        self.0.f64()
    }
}

/// Uniform permutation of `0..n` (Fisher–Yates). If `first` is given, that
/// index is placed at position 0 (the track the user clicked).
pub fn bag(n: usize, first: Option<usize>, rng: &mut Rng) -> Vec<usize> {
    let mut v: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        let j = rng.below(i + 1);
        v.swap(i, j);
    }
    pin_first(&mut v, first);
    v
}

/// Artist-balanced permutation of `0..artists.len()`; `artists[i]` is the
/// primary-artist key of track `i`.
pub fn spread(artists: &[String], first: Option<usize>, rng: &mut Rng) -> Vec<usize> {
    use std::collections::HashMap;
    let n = artists.len();
    let mut groups: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, a) in artists.iter().enumerate() {
        groups.entry(a.as_str()).or_default().push(i);
    }
    let mut keyed: Vec<(f64, usize)> = Vec::with_capacity(n);
    // Iterate groups in a stable order so a seeded rng gives stable output.
    let mut names: Vec<&str> = groups.keys().copied().collect();
    names.sort_unstable();
    for name in names {
        let mut idx = groups.remove(name).unwrap();
        // Shuffle within the artist so their own order is random too.
        for i in (1..idx.len()).rev() {
            let j = rng.below(i + 1);
            idx.swap(i, j);
        }
        let k = idx.len() as f64;
        let spacing = 1.0 / k;
        let offset = rng.f64() * spacing;
        for (m, t) in idx.into_iter().enumerate() {
            let jitter = (rng.f64() - 0.5) * spacing * 0.2;
            keyed.push((offset + m as f64 * spacing + jitter, t));
        }
    }
    keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut v: Vec<usize> = keyed.into_iter().map(|(_, t)| t).collect();
    pin_first(&mut v, first);
    v
}

fn pin_first(v: &mut [usize], first: Option<usize>) {
    if let Some(f) = first {
        if let Some(p) = v.iter().position(|&x| x == f) {
            // Rotate rather than swap so the rest of the spacing is preserved.
            v[..=p].rotate_right(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_perm(v: &[usize], n: usize) -> bool {
        let mut s = v.to_vec();
        s.sort_unstable();
        s == (0..n).collect::<Vec<_>>()
    }

    #[test]
    fn bag_is_permutation_and_pins_first() {
        let mut rng = Rng::seeded(7);
        for n in [0, 1, 2, 10, 500] {
            let first = if n > 0 { Some(n / 2) } else { None };
            let v = bag(n, first, &mut rng);
            assert!(is_perm(&v, n));
            if let Some(f) = first {
                assert_eq!(v[0], f);
            }
        }
    }

    #[test]
    fn bag_is_roughly_uniform() {
        // Position of element 0 over many shuffles of 4 should be ~uniform.
        let mut rng = Rng::seeded(1);
        let mut counts = [0u32; 4];
        for _ in 0..40_000 {
            let v = bag(4, None, &mut rng);
            counts[v.iter().position(|&x| x == 0).unwrap()] += 1;
        }
        for c in counts {
            assert!((9_000..11_000).contains(&c), "{counts:?}");
        }
    }

    #[test]
    fn spread_reduces_adjacent_same_artist() {
        // 3 artists, one dominant: A x10, B x5, C x5.
        let artists: Vec<String> = std::iter::repeat("A")
            .take(10)
            .chain(std::iter::repeat("B").take(5))
            .chain(std::iter::repeat("C").take(5))
            .map(String::from)
            .collect();
        let adj = |v: &[usize]| v.windows(2).filter(|w| artists[w[0]] == artists[w[1]]).count();
        let mut rng = Rng::seeded(42);
        let (mut s_total, mut b_total) = (0, 0);
        for _ in 0..200 {
            let s = spread(&artists, None, &mut rng);
            assert!(is_perm(&s, artists.len()));
            s_total += adj(&s);
            b_total += adj(&bag(artists.len(), None, &mut rng));
        }
        assert!(s_total * 2 < b_total, "spread {s_total} vs bag {b_total}");
    }
}
