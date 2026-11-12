//! Настройки движка: ядра процессора, кэш волновых оценок и параметры волновой модели.
//! Одна структура на всех: консоль и UCI меняют одни и те же значения.

use std::thread;

pub const THREADS_MIN: usize = 1;
pub const THREADS_MAX: usize = 256;

/// Размер кэша в мегабайтах; 0 — кэш выключен.
pub const CACHE_DEFAULT_MB: usize = 32;
pub const CACHE_MIN_MB: usize = 0;
pub const CACHE_MAX_MB: usize = 2048;

/// Вес волны угрозы (в процентах): насколько сильно движок боится «волнового размена»
/// на клетках своих фигур и ценит давление на чужие. 100 — как задумано, 0 — угрозы не учитываются.
pub const THREAT_DEFAULT: i32 = 130;
pub const THREAT_MIN: i32 = 0;
pub const THREAT_MAX: i32 = 300;

/// Вес прогноза (в процентах): сглаженные фазы, достоверность и исход цепочки.
/// 0 — только ближайшая волна.
pub const CHAIN_DEFAULT: i32 = 40;
pub const CHAIN_MIN: i32 = 0;
pub const CHAIN_MAX: i32 = 100;

/// Сколько логических ядер видит программа.
pub fn available_cores() -> usize {
    match thread::available_parallelism() {
        Ok(n) => n.get(),
        Err(_) => 1,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    pub threads: usize,
    pub cache_mb: usize,
    pub threat_pct: i32,
    pub chain_pct: i32,
    pub wave_width: usize,
    pub opening_variety: i32,
    pub random_seed: u64,
}

impl Default for EngineConfig {
    fn default() -> EngineConfig {
        let mut c = EngineConfig {
            threads: 1,
            cache_mb: CACHE_DEFAULT_MB,
            threat_pct: THREAT_DEFAULT,
            chain_pct: CHAIN_DEFAULT,
            wave_width: 8,
            opening_variety: 20,
            random_seed: 0,
        };
        c.set_threads(available_cores() as i64);
        c
    }
}

fn clamp_i64(v: i64, lo: i64, hi: i64) -> i64 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

impl EngineConfig {
    pub fn set_threads(&mut self, v: i64) {
        self.threads = clamp_i64(v, THREADS_MIN as i64, THREADS_MAX as i64) as usize;
    }

    pub fn set_cache_mb(&mut self, v: i64) {
        self.cache_mb = clamp_i64(v, CACHE_MIN_MB as i64, CACHE_MAX_MB as i64) as usize;
    }

    pub fn set_threat(&mut self, v: i64) {
        self.threat_pct = clamp_i64(v, THREAT_MIN as i64, THREAT_MAX as i64) as i32;
    }

    pub fn set_chain(&mut self, v: i64) {
        self.chain_pct = clamp_i64(v, CHAIN_MIN as i64, CHAIN_MAX as i64) as i32;
    }

    /// Текст для команды `settings` в консоли.
    pub fn describe(&self) -> String {
        let cache = if self.cache_mb == 0 {
            "выключен".to_string()
        } else {
            format!("{} МБ", self.cache_mb)
        };
        format!(
            "  ядра (потоки)         {}   (доступно в системе: {})\n  кэш волновых оценок   {}\n  вес волны угрозы      {} %\n  вес прогноза          {} %",
            self.threads,
            available_cores(),
            cache,
            self.threat_pct,
            self.chain_pct
        )
    }
}
