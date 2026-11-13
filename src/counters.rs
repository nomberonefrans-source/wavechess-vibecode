//! Worker-local atomic counters. Statistics must not serialize every wave on
//! one cache line when many cores share the field cache. Exact totals are read
//! at phase boundaries (or when a UCI node budget is explicitly requested).
use std::sync::atomic::{AtomicU64, Ordering};

#[repr(align(64))]
struct Shard([AtomicU64; 2]);

pub struct Counters([Shard; 64]);

impl Counters {
    pub fn new() -> Self {
        Self(std::array::from_fn(|_| {
            Shard(std::array::from_fn(|_| AtomicU64::new(0)))
        }))
    }

    #[inline]
    pub fn add(&self, index: usize) {
        let worker = rayon::current_thread_index().unwrap_or(0) % self.0.len();
        self.0[worker].0[index].fetch_add(1, Ordering::Relaxed);
    }

    pub fn values(&self) -> (u64, u64) {
        self.0.iter().fold((0, 0), |(a, b), shard| {
            (
                a + shard.0[0].load(Ordering::Relaxed),
                b + shard.0[1].load(Ordering::Relaxed),
            )
        })
    }

    pub fn clear(&self) {
        for shard in &self.0 {
            for counter in &shard.0 {
                counter.store(0, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rayon::prelude::*;

    #[test]
    fn parallel_totals_and_clear() {
        let counters = Counters::new();
        rayon::ThreadPoolBuilder::new()
            .num_threads(8)
            .build()
            .unwrap()
            .install(|| {
                (0..10000).into_par_iter().for_each(|i| {
                    counters.add(0);
                    if i % 2 == 0 {
                        counters.add(1);
                    }
                });
            });
        assert_eq!(counters.values(), (10000, 5000));
        counters.clear();
        assert_eq!(counters.values(), (0, 0));
    }
}
