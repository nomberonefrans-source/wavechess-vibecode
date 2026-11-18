//! Ограниченная память осевших волн. Здесь только точные значения поля:
//! нет глубин, границ поиска и оценок предсказанных цепочек.
//!
//! Четыре записи на корзину уменьшают вытеснения при совпадении индекса.
//! Пара ключ/значение читается под версионной меткой: одновременная запись
//! может дать промах, но не чужую оценку. Все атомики снимка используют
//! SeqCst; статистика и выбор места для вытеснения не участвуют в снимке.

use crate::counters::Counters;
use rayon::prelude::*;
use std::mem::size_of;
use std::sync::atomic::{AtomicU64, Ordering};

const WAYS: usize = 4;
const MAX_MB: usize = 2048;

struct Entry {
    sequence: AtomicU64,
    key: AtomicU64,
    data: AtomicU64,
}

impl Entry {
    fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            key: AtomicU64::new(0),
            data: AtomicU64::new(0),
        }
    }

    /// None означает занятую или изменившуюся запись. Пустая запись имеет data = 0.
    fn snapshot(&self) -> Option<(u64, u64)> {
        let before = self.sequence.load(Ordering::SeqCst);
        if before & 1 != 0 {
            return None;
        }
        let key = self.key.load(Ordering::SeqCst);
        let data = self.data.load(Ordering::SeqCst);
        if self.sequence.load(Ordering::SeqCst) != before {
            return None;
        }
        Some((key, data))
    }

    /// Одна попытка: вычисление волн не ждёт конкурирующего писателя.
    fn write(&self, key: u64, data: u64) -> bool {
        let sequence = self.sequence.load(Ordering::SeqCst);
        if sequence & 1 != 0
            || self
                .sequence
                .compare_exchange(
                    sequence,
                    sequence.wrapping_add(1),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_err()
        {
            return false;
        }
        self.key.store(key, Ordering::SeqCst);
        self.data.store(data, Ordering::SeqCst);
        self.sequence
            .store(sequence.wrapping_add(2), Ordering::SeqCst);
        true
    }
}

#[repr(align(64))]
struct Bucket {
    entries: [Entry; WAYS],
    next: AtomicU64,
}

impl Bucket {
    fn new() -> Self {
        Self {
            entries: std::array::from_fn(|_| Entry::new()),
            next: AtomicU64::new(0),
        }
    }
}

pub struct WaveCache {
    buckets: Vec<Bucket>,
    generation: u32,
    counters: Counters,
}

impl WaveCache {
    pub fn new(mb: usize) -> Self {
        let count = mb.min(MAX_MB) * (1024 * 1024 / size_of::<Bucket>());
        let buckets = if mb >= 64 {
            (0..count).into_par_iter().map(|_| Bucket::new()).collect()
        } else {
            (0..count).map(|_| Bucket::new()).collect()
        };
        Self {
            buckets,
            generation: 1,
            counters: Counters::new(rayon::current_num_threads()),
        }
    }

    fn bucket(&self, key: u64) -> &Bucket {
        let index = (key as u128 * self.buckets.len() as u128) >> 64;
        &self.buckets[index as usize]
    }

    pub fn get(&self, key: u64) -> Option<i32> {
        if !self.is_enabled() {
            return None;
        }
        self.counters.add(0);
        for entry in &self.bucket(key).entries {
            if let Some((stored_key, data)) = entry.snapshot() {
                if stored_key == key && data >> 32 == self.generation as u64 {
                    self.counters.add(1);
                    return Some(data as u32 as i32);
                }
            }
        }
        None
    }

    pub fn put(&self, key: u64, value: i32) {
        if !self.is_enabled() {
            return;
        }
        let bucket = self.bucket(key);
        let data = ((self.generation as u64) << 32) | value as u32 as u64;
        let mut empty = None;
        for (i, entry) in bucket.entries.iter().enumerate() {
            if let Some((stored_key, old_data)) = entry.snapshot() {
                if old_data >> 32 != self.generation as u64 {
                    empty.get_or_insert(i);
                } else if stored_key == key {
                    // При гонке допустим промах: значение можно вычислить повторно.
                    let _ = entry.write(key, data);
                    return;
                }
            }
        }
        let first =
            empty.unwrap_or_else(|| bucket.next.fetch_add(1, Ordering::Relaxed) as usize % WAYS);
        for offset in 0..WAYS {
            if bucket.entries[(first + offset) % WAYS].write(key, data) {
                return;
            }
        }
    }

    /// Вызывать между расчётами, после присоединения рабочих потоков.
    pub fn clear(&mut self) {
        if self.generation == u32::MAX {
            for bucket in &mut self.buckets {
                *bucket = Bucket::new();
            }
            self.generation = 1;
        } else {
            self.generation += 1;
        }
        self.counters.clear();
    }

    pub fn is_enabled(&self) -> bool {
        !self.buckets.is_empty()
    }

    pub fn entries(&self) -> usize {
        self.buckets.len() * WAYS
    }

    /// Выделенный массив, включая версии записей и указатели вытеснения.
    pub fn bytes(&self) -> usize {
        self.buckets.len() * size_of::<Bucket>()
    }

    pub fn resize_counters(&mut self, workers: usize) {
        self.counters = Counters::new(workers);
    }

    pub fn stats(&self) -> (u64, u64) {
        self.counters.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn disabled_and_memory_budget() {
        let cache = WaveCache::new(0);
        cache.put(0, -1);
        assert_eq!(cache.get(0), None);
        assert_eq!(cache.stats(), (0, 0));
        assert_eq!(cache.entries(), 0);
        assert_eq!(cache.bytes(), 0);
        for mb in [1, 2, 3, 16] {
            let cache = WaveCache::new(mb);
            assert!(cache.is_enabled());
            assert!(cache.bytes() <= mb * 1024 * 1024);
            assert_eq!(cache.bytes(), mb * 1024 * 1024);
        }
    }

    #[test]
    fn full_keys_signed_scores_and_clear() {
        let mut cache = WaveCache::new(1);
        for (key, score) in [(0, 0), (u64::MAX, i32::MIN), (1, i32::MAX), (2, -30000)] {
            assert_eq!(cache.get(key), None);
            cache.put(key, score);
            assert_eq!(cache.get(key), Some(score));
        }
        cache.put(0, -17);
        assert_eq!(cache.get(0), Some(-17));
        cache.clear();
        assert_eq!(cache.stats(), (0, 0));
        for key in [0, u64::MAX, 1, 2] {
            assert_eq!(cache.get(key), None);
        }
        cache.put(0, 23);
        assert_eq!(cache.get(0), Some(23));
    }

    #[test]
    fn generation_wrap_drops_old_entries() {
        let mut cache = WaveCache::new(1);
        cache.put(0, 17);
        cache.generation = u32::MAX;
        cache.put(u64::MAX, 23);
        cache.clear();
        assert_eq!(cache.get(0), None);
        assert_eq!(cache.get(u64::MAX), None);
    }

    #[test]
    fn four_colliding_positions_coexist() {
        let cache = WaveCache::new(1);
        for i in 0..WAYS {
            cache.put(i as u64, i as i32 - 19);
        }
        for i in 0..WAYS {
            assert_eq!(cache.get(i as u64), Some(i as i32 - 19));
        }
        cache.put(WAYS as u64, 91);
        assert_eq!(cache.get(WAYS as u64), Some(91));
        assert_eq!(cache.get(0), None);
        assert_eq!(cache.get(1), Some(-18));
    }

    #[test]
    fn concurrent_collisions_never_return_another_positions_score() {
        let cache = Arc::new(WaveCache::new(1));
        let barrier = Arc::new(Barrier::new(8));
        std::thread::scope(|scope| {
            for worker in 0..8u64 {
                let cache = Arc::clone(&cache);
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    for iteration in 0..20000u64 {
                        let id = (iteration * 13 + worker * 7) % 64;
                        let key = id;
                        let value = id as i32 * 997 - 31000;
                        cache.put(key, value);
                        if let Some(actual) = cache.get(key) {
                            assert_eq!(actual, value, "mixed cache entry for key {key}");
                        }
                        let other = (id + 29) % 64;
                        if let Some(actual) = cache.get(other) {
                            assert_eq!(actual, other as i32 * 997 - 31000);
                        }
                    }
                });
            }
        });
    }
}
