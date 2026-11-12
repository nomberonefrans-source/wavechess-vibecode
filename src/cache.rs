//! Ограниченная память осевших волн. Здесь только точные значения поля:
//! нет глубин, границ поиска и оценок предсказанных цепочек.
//!
//! Четыре записи на корзину уменьшают вытеснения при совпадении индекса.
//! Пара ключ/значение читается под версионной меткой: одновременная запись
//! может дать промах, но не чужую оценку. Все атомики снимка используют
//! SeqCst; статистика и выбор места для вытеснения не участвуют в снимке.

use std::mem::size_of;
use std::sync::atomic::{AtomicU64, Ordering};

const WAYS: usize = 4;
const VALID_BIT: u64 = 1 << 32;
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
    mask: usize,
    probes: AtomicU64,
    hits: AtomicU64,
}

impl WaveCache {
    pub fn new(mb: usize) -> Self {
        let target = mb.min(MAX_MB) * 1024 * 1024 / size_of::<Bucket>();
        let count = if target == 0 {
            0
        } else {
            1usize << (usize::BITS - 1 - target.leading_zeros())
        };
        let buckets = (0..count).map(|_| Bucket::new()).collect();
        Self {
            buckets,
            mask: count.saturating_sub(1),
            probes: AtomicU64::new(0),
            hits: AtomicU64::new(0),
        }
    }

    pub fn get(&self, key: u64) -> Option<i32> {
        if !self.is_enabled() {
            return None;
        }
        self.probes.fetch_add(1, Ordering::Relaxed);
        for entry in &self.buckets[(key as usize) & self.mask].entries {
            if let Some((stored_key, data)) = entry.snapshot() {
                if stored_key == key && data & VALID_BIT != 0 {
                    self.hits.fetch_add(1, Ordering::Relaxed);
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
        let bucket = &self.buckets[(key as usize) & self.mask];
        let data = VALID_BIT | value as u32 as u64;
        let mut empty = None;
        for (i, entry) in bucket.entries.iter().enumerate() {
            if let Some((stored_key, old_data)) = entry.snapshot() {
                if old_data & VALID_BIT == 0 {
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
    /// Версии сохраняют монотонность даже при очистке; обнуление версий
    /// позволило бы читателю принять запись после очистки за старую.
    pub fn clear(&self) {
        for bucket in &self.buckets {
            for entry in &bucket.entries {
                while !entry.write(0, 0) {
                    std::thread::yield_now();
                }
            }
            bucket.next.store(0, Ordering::Relaxed);
        }
        self.probes.store(0, Ordering::Relaxed);
        self.hits.store(0, Ordering::Relaxed);
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

    pub fn stats(&self) -> (u64, u64) {
        (
            self.probes.load(Ordering::Relaxed),
            self.hits.load(Ordering::Relaxed),
        )
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
            assert!(cache.bytes() > mb * 1024 * 1024 / 2);
        }
    }

    #[test]
    fn full_keys_signed_scores_and_clear() {
        let cache = WaveCache::new(1);
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
    fn four_colliding_positions_coexist() {
        let cache = WaveCache::new(1);
        let step = cache.buckets.len() as u64;
        for i in 0..WAYS {
            cache.put(i as u64 * step, i as i32 - 19);
        }
        for i in 0..WAYS {
            assert_eq!(cache.get(i as u64 * step), Some(i as i32 - 19));
        }
        cache.put(WAYS as u64 * step, 91);
        assert_eq!(cache.get(WAYS as u64 * step), Some(91));
        assert_eq!(cache.get(0), None);
        assert_eq!(cache.get(step), Some(-18));
    }

    #[test]
    fn concurrent_collisions_never_return_another_positions_score() {
        let cache = Arc::new(WaveCache::new(1));
        let barrier = Arc::new(Barrier::new(8));
        let step = cache.buckets.len() as u64;
        std::thread::scope(|scope| {
            for worker in 0..8u64 {
                let cache = Arc::clone(&cache);
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    for iteration in 0..20000u64 {
                        let id = (iteration * 13 + worker * 7) % 64;
                        let key = id * step;
                        let value = id as i32 * 997 - 31000;
                        cache.put(key, value);
                        if let Some(actual) = cache.get(key) {
                            assert_eq!(actual, value, "mixed cache entry for key {key}");
                        }
                        let other = (id + 29) % 64;
                        if let Some(actual) = cache.get(other * step) {
                            assert_eq!(actual, other as i32 * 997 - 31000);
                        }
                    }
                });
            }
        });
    }
}
