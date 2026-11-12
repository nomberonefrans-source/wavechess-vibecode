//! Протокол UCI + настройки движка (UCI options) — всё в одном файле.
//!
//! Настройки, которые увидит оболочка (Arena, Cute Chess, Lichess-bot, ChessBase, En Croissant ...):
//!   Threads           число ядер (потоков) для расчёта ходов; по умолчанию — все логические ядра
//!   Hash              размер кэша волновых оценок, МБ (0 — кэш выключен, по умолчанию 32)
//!   Clear Hash        кнопка: очистить кэш
//!   MultiPV           число линий анализа 1..8 (старое имя `Variants` тоже принимается)
//!   Threat Weight     вес волны угрозы, % (0..300, по умолчанию 130)
//!   Chain Weight      вес конца волновой цепочки, % (0..100, по умолчанию 40)
//!   Move Overhead     запас времени на задержки оболочки/сети, мс (вычитается из времени хода)
//!   UCI_LimitStrength / UCI_Elo   ограничение силы (длина цепочки + число позиций, шкала ПРИБЛИЗИТЕЛЬНАЯ)
//!   UCI_AnalyseMode   режим анализа: ограничение силы отключено, план партии не запоминается
//!
//! Что сделано, чтобы не конфликтовать с оболочками:
//!   * значение `setoption ... value ...` читается целиком, с пробелами, имя — без учёта регистра;
//!   * неизвестные опции молча игнорируются (никакого мусора в stdout);
//!   * `isready` во время расчёта отвечает сразу, а не после окончания хода;
//!   * `quit` и закрытие stdin останавливают расчёт (`go infinite` не остаётся процессом-сиротой);
//!   * `go infinite` не выдаёт `bestmove`, пока не придёт `stop` (даже если найден мат);
//!   * учитываются `movestogo` и `Move Overhead`; бюджет ограничивается остатком на часах;
//!   * ввод читается побайтово, неправильная кодировка не обрывает связь с оболочкой.
//!
//! Ponder не объявляется намеренно: расчёт идёт в основном потоке, `go ponder`/`ponderhit`
//! не поддерживаются, а оболочка не пришлёт `go ponder`, если опции Ponder нет.

use crate::board::*;
use crate::engine::*;
use crate::search::*;
use crate::settings::*;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

// ---------------------------------------------------------------- настройки

const MULTIPV_MAX: usize = 8;
const OVERHEAD_DEFAULT: i64 = 30;
const OVERHEAD_MAX: i64 = 5000;
const ELO_MIN: i32 = 800;
const ELO_MAX: i32 = 2400;
const ELO_DEFAULT: i32 = 1500;

/// Настройки протокола. Ядра, кэш и волновые веса живут в `EngineConfig` самого движка.
struct Settings {
    multipv: usize,
    move_overhead: i64,
    limit_strength: bool,
    elo: i32,
    analyse: bool,
}

impl Settings {
    fn new() -> Settings {
        Settings {
            multipv: 1,
            move_overhead: OVERHEAD_DEFAULT,
            limit_strength: false,
            elo: ELO_DEFAULT,
            analyse: false,
        }
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

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_lowercase().as_str() {
        "true" | "1" | "on" | "yes" => Some(true),
        "false" | "0" | "off" | "no" => Some(false),
        _ => None,
    }
}

/// Elo -> (предел длины цепочки, предел числа осевших позиций). Шкала приблизительная, по рейтингам не калибровалась.
fn strength_limits(elo: i32) -> (i32, u64) {
    let e = elo.max(ELO_MIN).min(ELO_MAX);
    let t = (e - ELO_MIN) as f64 / (ELO_MAX - ELO_MIN) as f64;
    let depth = 1 + (t * (crate::search::DEFAULT_DEPTH - 1) as f64).round() as i32;
    let nodes = (1000.0 * (2.0f64).powf(t * 12.0)) as u64;
    (depth, nodes)
}

fn print_options() {
    out(&format!(
        "option name Threads type spin default {} min {} max {}",
        EngineConfig::default().threads,
        THREADS_MIN,
        THREADS_MAX
    ));
    out(&format!(
        "option name Hash type spin default {} min {} max {}",
        CACHE_DEFAULT_MB, CACHE_MIN_MB, CACHE_MAX_MB
    ));
    out("option name Clear Hash type button");
    out("option name Wave Width type spin default 8 min 1 max 32");
    out("option name Opening Variety type spin default 20 min 0 max 100");
    out("option name Random Seed type spin default 0 min 0 max 2147483647");
    out(&format!(
        "option name MultiPV type spin default 1 min 1 max {}",
        MULTIPV_MAX
    ));
    out(&format!(
        "option name Threat Weight type spin default {} min {} max {}",
        THREAT_DEFAULT, THREAT_MIN, THREAT_MAX
    ));
    out(&format!(
        "option name Chain Weight type spin default {} min {} max {}",
        CHAIN_DEFAULT, CHAIN_MIN, CHAIN_MAX
    ));
    out(&format!(
        "option name Move Overhead type spin default {} min 0 max {}",
        OVERHEAD_DEFAULT, OVERHEAD_MAX
    ));
    out("option name UCI_LimitStrength type check default false");
    out(&format!(
        "option name UCI_Elo type spin default {} min {} max {}",
        ELO_DEFAULT, ELO_MIN, ELO_MAX
    ));
    out("option name UCI_AnalyseMode type check default false");
    for (name, value) in crate::params::values() {
        out(&format!(
            "option name W:{} type string default {}",
            name, value
        ));
    }
}

fn apply_option(name: &str, value: &str, st: &mut Settings, eng: &mut Engine) {
    match name.trim().to_lowercase().as_str() {
        "wave width" | "opening variety" | "random seed" => {
            if let Ok(v) = value.trim().parse::<i64>() {
                let mut c = eng.config();
                match name.trim().to_lowercase().as_str() {
                    "wave width" => c.wave_width = v.clamp(1, 32) as usize,
                    "opening variety" => c.opening_variety = v.clamp(0, 100) as i32,
                    _ => c.random_seed = v.clamp(0, 2147483647) as u64,
                }
                eng.set_config(&c);
            }
        }
        "threads" => {
            if let Ok(v) = value.trim().parse::<i64>() {
                let mut c = eng.config();
                c.set_threads(v);
                eng.set_config(&c);
            }
        }
        "hash" | "cache" => {
            if let Ok(v) = value.trim().parse::<i64>() {
                let mut c = eng.config();
                c.set_cache_mb(v);
                eng.set_config(&c);
            }
        }
        "clear hash" | "clear cache" => {
            eng.searcher.clear();
        }
        "threat weight" => {
            if let Ok(v) = value.trim().parse::<i64>() {
                let mut c = eng.config();
                c.set_threat(v);
                eng.set_config(&c);
            }
        }
        "chain weight" => {
            if let Ok(v) = value.trim().parse::<i64>() {
                let mut c = eng.config();
                c.set_chain(v);
                eng.set_config(&c);
            }
        }
        "multipv" | "variants" => {
            if let Ok(v) = value.trim().parse::<i64>() {
                st.multipv = clamp_i64(v, 1, MULTIPV_MAX as i64) as usize;
            }
        }
        "move overhead" => {
            if let Ok(v) = value.trim().parse::<i64>() {
                st.move_overhead = clamp_i64(v, 0, OVERHEAD_MAX);
            }
        }
        "uci_limitstrength" => {
            if let Some(b) = parse_bool(value) {
                st.limit_strength = b;
            }
        }
        "uci_elo" => {
            if let Ok(v) = value.trim().parse::<i64>() {
                st.elo = clamp_i64(v, ELO_MIN as i64, ELO_MAX as i64) as i32;
            }
        }
        "uci_analysemode" => {
            if let Some(b) = parse_bool(value) {
                st.analyse = b;
            }
        }
        n if n.starts_with("w:") => {
            // подбор весов волновой модели (см. params.rs); кэш осевших позиций при этом устаревает
            if let Ok(v) = value.trim().parse::<f32>() {
                if crate::params::set(&n[2..], v) {
                    eng.searcher.clear();
                }
            }
        }
        // неизвестные опции игнорируются молча
        _ => {}
    }
}

// ---------------------------------------------------------------- протокол

/// Строка в stdout с немедленным сбросом буфера; ошибки записи (оболочка закрыта) не паникуют.
fn out(s: &str) {
    let stdout = io::stdout();
    let mut h = stdout.lock();
    let _ = writeln!(h, "{}", s);
    let _ = h.flush();
}

fn find_uci_move(b: &Board, text: &str) -> Option<Move> {
    let legal = b.legal_moves();
    for i in 0..legal.n {
        if legal.m[i].to_uci() == text {
            return Some(legal.m[i]);
        }
    }
    None
}

pub fn run() {
    let (tx, rx) = mpsc::channel::<(String, u64)>();
    let stop = Arc::new(AtomicBool::new(false));
    let searching = Arc::new(AtomicBool::new(false));
    // Отмена привязана к номеру go: очистка stop после старого расчёта
    // не должна терять уже прочитанный stop для следующего go.
    let active = Arc::new(AtomicU64::new(0));
    let cancelled = Arc::new(AtomicU64::new(0));

    // Поток чтения stdin: сам обрабатывает то, что нельзя откладывать до конца поиска.
    let stop_reader = stop.clone();
    let searching_reader = searching.clone();
    let active_reader = active.clone();
    let cancelled_reader = cancelled.clone();
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut rd = stdin.lock();
        let mut buf: Vec<u8> = Vec::new();
        let mut sequence = 0u64;
        loop {
            buf.clear();
            match rd.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let t = String::from_utf8_lossy(&buf).trim().to_string();
            let first = t.split_whitespace().next().unwrap_or("");
            if first == "quit" {
                cancelled_reader.store(u64::MAX, Ordering::SeqCst);
                stop_reader.store(true, Ordering::SeqCst);
            } else if first == "stop" {
                cancelled_reader.fetch_max(sequence, Ordering::SeqCst);
                let running = active_reader.load(Ordering::SeqCst);
                if running != 0 && running <= sequence {
                    stop_reader.store(true, Ordering::SeqCst);
                }
            } else if first == "go" {
                sequence = sequence.saturating_add(1);
                searching_reader.store(true, Ordering::SeqCst);
            }
            if first == "isready" && searching_reader.load(Ordering::SeqCst) {
                out("readyok");
                continue;
            }
            if tx.send((t, sequence)).is_err() {
                break;
            }
        }
        // Закрытие stdin отменяет также go, ещё стоящие в очереди.
        cancelled_reader.store(u64::MAX, Ordering::SeqCst);
        stop_reader.store(true, Ordering::SeqCst);
    });

    let mut eng = Engine::new(stop.clone());
    let mut st = Settings::new();
    let mut start_fen = String::new();
    let mut played: Vec<String> = Vec::new();
    let mut engine_side: i8 = 0;

    loop {
        let (line, search_id) = match rx.recv() {
            Ok(l) => l,
            Err(_) => break,
        };
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        match words[0] {
            "uci" => {
                out("id name WaveChess 3.1 (wave mathematics)");
                out("id author WaveChess");
                print_options();
                out("uciok");
            }
            "isready" => {
                out("readyok");
            }
            "ucinewgame" => {
                eng.new_game();
                start_fen.clear();
                played.clear();
                engine_side = 0;
            }
            "setoption" => {
                let mut name = String::new();
                let mut value = String::new();
                let mut mode = 0;
                for w in words.iter().skip(1) {
                    if mode == 0 && *w == "name" {
                        mode = 1;
                        continue;
                    }
                    if mode == 1 && *w == "value" {
                        mode = 2;
                        continue;
                    }
                    if mode == 1 {
                        if !name.is_empty() {
                            name.push(' ');
                        }
                        name.push_str(w);
                    } else if mode == 2 {
                        if !value.is_empty() {
                            value.push(' ');
                        }
                        value.push_str(w);
                    }
                }
                apply_option(&name, &value, &mut st, &mut eng);
            }
            "position" => {
                let rest: Vec<&str> = words.iter().skip(1).cloned().collect();
                let mut i: usize;
                let fen: String;
                if rest.get(0) == Some(&"startpos") {
                    fen = Board::START_FEN.to_string();
                    i = 1;
                } else if rest.get(0) == Some(&"fen") {
                    let mut parts: Vec<&str> = Vec::new();
                    i = 1;
                    while i < rest.len() && rest[i] != "moves" {
                        parts.push(rest[i]);
                        i += 1;
                    }
                    fen = parts.join(" ");
                } else {
                    continue;
                }
                let mut moves: Vec<String> = Vec::new();
                if i < rest.len() && rest[i] == "moves" {
                    for t in rest.iter().skip(i + 1) {
                        moves.push(t.to_string());
                    }
                }
                let can_extend = fen == start_fen
                    && moves.len() >= played.len()
                    && moves[..played.len()] == played[..];
                if !can_extend {
                    match Board::from_fen(&fen) {
                        Ok(b) => {
                            eng.sync_board(b, engine_side);
                            start_fen = fen.clone();
                            played.clear();
                        }
                        Err(_) => continue,
                    }
                }
                for idx in played.len()..moves.len() {
                    match find_uci_move(&eng.board, &moves[idx]) {
                        Some(m) => {
                            let by_engine = eng.board.side == engine_side;
                            eng.play(m, by_engine);
                            played.push(moves[idx].clone());
                        }
                        None => break,
                    }
                }
            }
            "go" => {
                let mut depth: i32 = 0;
                let mut movetime: i64 = 0;
                let mut wtime: i64 = 0;
                let mut btime: i64 = 0;
                let mut winc: i64 = 0;
                let mut binc: i64 = 0;
                let mut movestogo: i64 = 0;
                let mut have_clock = false;
                let mut nodes: u64 = 0;
                let mut infinite = false;
                let mut i = 1usize;
                while i < words.len() {
                    match words[i] {
                        "depth" => {
                            if i + 1 < words.len() {
                                depth = words[i + 1].parse::<i32>().unwrap_or(0);
                                i += 1;
                            }
                        }
                        "movetime" => {
                            if i + 1 < words.len() {
                                movetime = words[i + 1].parse::<i64>().unwrap_or(0);
                                i += 1;
                            }
                        }
                        "nodes" => {
                            if i + 1 < words.len() {
                                nodes = words[i + 1].parse::<u64>().unwrap_or(0);
                                i += 1;
                            }
                        }
                        "movestogo" => {
                            if i + 1 < words.len() {
                                movestogo = words[i + 1].parse::<i64>().unwrap_or(0);
                                i += 1;
                            }
                        }
                        "infinite" => {
                            infinite = true;
                        }
                        "wtime" => {
                            have_clock = true;
                            if i + 1 < words.len() {
                                wtime = words[i + 1].parse::<i64>().unwrap_or(0);
                                i += 1;
                            }
                        }
                        "btime" => {
                            have_clock = true;
                            if i + 1 < words.len() {
                                btime = words[i + 1].parse::<i64>().unwrap_or(0);
                                i += 1;
                            }
                        }
                        "winc" => {
                            if i + 1 < words.len() {
                                winc = words[i + 1].parse::<i64>().unwrap_or(0);
                                i += 1;
                            }
                        }
                        "binc" => {
                            if i + 1 < words.len() {
                                binc = words[i + 1].parse::<i64>().unwrap_or(0);
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }

                let side = eng.board.side;
                engine_side = side;
                let (my_time, my_inc) = if side == 1 {
                    (wtime, winc)
                } else {
                    (btime, binc)
                };
                let overhead = st.move_overhead;

                let mut time_ms: u64 = 0;
                let mut max_depth: i32 = crate::search::DEFAULT_DEPTH;
                if infinite {
                    max_depth = crate::search::MAX_DEPTH;
                    // Цепочки растут до лимита модели; bestmove ждёт stop.
                } else if movetime > 0 {
                    let t = movetime.saturating_sub(overhead);
                    time_ms = if t < 1 { 1 } else { t as u64 };
                } else if have_clock {
                    let mut mt = my_time.saturating_sub(overhead);
                    if mt < 1 {
                        mt = 1;
                    }
                    let div: i64 = if movestogo > 0 {
                        (if movestogo > 40 { 40 } else { movestogo }) + 1
                    } else {
                        28
                    };
                    let mut t = (mt / div).saturating_add(my_inc.max(0) / 2);
                    if t > mt / 3 {
                        t = mt / 3;
                    }
                    if t < 20 {
                        t = 20;
                    }
                    if t > mt {
                        t = mt;
                    }
                    if t < 1 {
                        t = 1;
                    }
                    time_ms = t as u64;
                }
                if depth > 0 {
                    max_depth = depth;
                }
                let mut node_cap: u64 = nodes;
                if st.limit_strength && !st.analyse {
                    let (lim_depth, lim_nodes) = strength_limits(st.elo);
                    if lim_depth < max_depth {
                        max_depth = lim_depth;
                    }
                    if node_cap == 0 || lim_nodes < node_cap {
                        node_cap = lim_nodes;
                    }
                }
                // `go` без каких-либо лимитов (кроме infinite) не должен считать вечно
                if !infinite && time_ms == 0 && depth == 0 && nodes == 0 {
                    time_ms = 2000;
                }
                eng.searcher.node_limit = node_cap;
                let limits = SearchLimits {
                    max_depth,
                    time_ms,
                    variants: st.multipv,
                };
                stop.store(false, Ordering::SeqCst);
                active.store(search_id, Ordering::SeqCst);
                if cancelled.load(Ordering::SeqCst) >= search_id {
                    stop.store(true, Ordering::SeqCst);
                }
                let res = eng.think(&limits, Report::Uci, !st.analyse);

                // По протоколу `go infinite` отвечает bestmove только после `stop`
                if infinite {
                    while !stop.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(2));
                    }
                }
                active.store(0, Ordering::SeqCst);
                stop.store(false, Ordering::SeqCst);

                if res.best.is_none() {
                    out("bestmove 0000");
                } else {
                    let mut s = format!("bestmove {}", res.best.to_uci());
                    if let Some(l) = res.lines.first() {
                        if l.pv.len() > 1 {
                            s.push_str(&format!(" ponder {}", l.pv[1].to_uci()));
                        }
                    }
                    out(&s);
                }
                searching.store(false, Ordering::SeqCst);
            }
            "stop" => {} // уже учтён потоком чтения по номеру go
            "d" | "fen" => {
                out(&eng.board.to_fen());
            }
            "memory" => {
                out(&format!(
                    "info string memory moves {} positions {} predictions {}/{} plan {}",
                    eng.history.len(),
                    eng.hashes.len(),
                    eng.predicted_hits,
                    eng.predicted_total,
                    eng.plan
                        .iter()
                        .map(Move::to_uci)
                        .collect::<Vec<_>>()
                        .join(" ")
                ));
            }
            "quit" => break,
            _ => {}
        }
    }
}
