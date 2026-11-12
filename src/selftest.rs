//! Самопроверка: правила (perft), симметрия волновой оценки, тактика, запись ходов.

use crate::board::*;
use crate::cache::WaveCache;
use crate::notation;
use crate::search::*;
use crate::settings::*;
use crate::wave;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

pub fn perft_cmd(args: &[String]) {
    let depth: u32 = args.get(0).and_then(|s| s.parse::<u32>().ok()).unwrap_or(4);
    let fen = if args.len() > 1 {
        args[1..].join(" ")
    } else {
        Board::START_FEN.to_string()
    };
    let b = match Board::from_fen(&fen) {
        Ok(b) => b,
        Err(e) => {
            println!("Ошибка: {}", e);
            return;
        }
    };
    let t0 = Instant::now();
    let n = b.perft(depth);
    println!(
        "perft({}) = {}  [{:.2} с]",
        depth,
        n,
        t0.elapsed().as_secs_f32()
    );
}

fn check_perft(name: &str, fen: &str, expected: &[u64]) -> bool {
    let b = Board::from_fen(fen).unwrap();
    let mut ok = true;
    let mut got: Vec<u64> = Vec::new();
    for (i, &e) in expected.iter().enumerate() {
        let n = b.perft((i + 1) as u32);
        got.push(n);
        if n != e {
            ok = false;
        }
    }
    println!(
        "  [{}] правила, {:<28} {:?}",
        if ok { "OK " } else { "ОШИБКА" },
        name,
        got
    );
    ok
}

fn best_move_with(fen: &str, depth: i32, threads: i64, cache_mb: i64) -> (String, i32) {
    let stop = Arc::new(AtomicBool::new(false));
    let mut cfg = EngineConfig::default();
    cfg.set_threads(threads);
    cfg.set_cache_mb(cache_mb);
    let mut s = Searcher::new(&cfg, stop);
    let b = Board::from_fen(fen).unwrap();
    let limits = SearchLimits {
        max_depth: depth,
        time_ms: 0,
        variants: 1,
    };
    let res = s.think(&b, &[b.hash], &limits, Move::NONE, Report::Quiet);
    (res.best.to_uci(), res.score)
}

/// Полный результат поиска (с цепочкой) для проверок тактики.
fn full_search(fen: &str, depth: i32) -> SearchResult {
    let stop = Arc::new(AtomicBool::new(false));
    let mut cfg = EngineConfig::default();
    cfg.set_threads(1);
    cfg.set_cache_mb(16);
    let mut s = Searcher::new(&cfg, stop);
    let b = Board::from_fen(fen).unwrap();
    let limits = SearchLimits {
        max_depth: depth,
        time_ms: 0,
        variants: 1,
    };
    s.think(&b, &[b.hash], &limits, Move::NONE, Report::Quiet)
}

fn best_move(fen: &str, depth: i32) -> (String, i32) {
    best_move_with(fen, depth, 1, 16)
}

pub fn run() -> bool {
    let mut all_ok = true;
    println!("WaveChess: самопроверка");

    println!("1) Правила шахмат (подсчёт узлов, эталонные значения):");
    all_ok &= check_perft(
        "начальная позиция",
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
        &[20, 400, 8902, 197281],
    );
    all_ok &= check_perft(
        "Kiwipete (рокировки, шахи)",
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        &[48, 2039, 97862],
    );
    all_ok &= check_perft(
        "эндшпиль (взятие на проходе)",
        "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
        &[14, 191, 2812, 43238],
    );
    all_ok &= check_perft(
        "превращения и шахи",
        "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
        &[6, 264, 9467],
    );
    all_ok &= check_perft(
        "превращения со взятием",
        "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
        &[44, 1486, 62379],
    );
    all_ok &= check_perft(
        "миттельшпиль",
        "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
        &[46, 2079, 89890],
    );

    // инкрементальный отпечаток позиции совпадает с полным пересчётом на всех ходах дерева
    fn hash_walk(b: &Board, depth: u32) -> bool {
        if depth == 0 {
            return true;
        }
        let l = b.legal_moves();
        for i in 0..l.n {
            let nb = b.make(l.m[i]);
            if nb.hash != nb.compute_hash() || !hash_walk(&nb, depth - 1) {
                return false;
            }
        }
        true
    }
    let mut hash_ok = true;
    for f in [
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
        "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
    ] {
        hash_ok &= hash_walk(&Board::from_fen(f).unwrap(), 3);
    }
    println!(
        "  [{}] хэш позиции при ходах (рокировка, взятие на проходе, превращение)",
        if hash_ok { "OK " } else { "ОШИБКА" }
    );
    all_ok &= hash_ok;

    println!("2) Симметрия волновой оценки (позиция и её зеркало дают противоположные баланс):");
    let sym_fens = [
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
        "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
    ];
    let mut sym_ok = true;
    for f in sym_fens.iter() {
        let b = Board::from_fen(f).unwrap();
        let a = wave::evaluate(&b);
        let m = wave::evaluate(&b.mirrored());
        if (a + m).abs() > 1 {
            sym_ok = false;
        }
    }
    println!(
        "  [{}] симметрия оценки",
        if sym_ok { "OK " } else { "ОШИБКА" }
    );
    all_ok &= sym_ok;

    // осевшая волна тоже симметрична: позиция и её зеркало дают одну и ту же оценку для сходившего
    let mut settle_ok = true;
    for f in sym_fens.iter() {
        let b = Board::from_fen(f).unwrap();
        let a = settle_value(&b, 100);
        let m = settle_value(&b.mirrored(), 100);
        if (a - m).abs() > 2 {
            settle_ok = false;
        }
    }
    println!(
        "  [{}] симметрия осевшей волны (с волной угрозы)",
        if settle_ok { "OK " } else { "ОШИБКА" }
    );
    all_ok &= settle_ok;

    println!("3) Тактика (волновые траектории):");
    let (mv, sc) = best_move("6k1/5ppp/8/8/8/8/8/R5K1 w - - 0 1", 4);
    let t1 = mv == "a1a8" && sc > MATE_BOUND;
    println!(
        "  [{}] мат в 1: найден {} (оценка {})",
        if t1 { "OK " } else { "ОШИБКА" },
        mv,
        sc
    );
    all_ok &= t1;
    let (mv, _) = best_move(
        "rnb1kbnr/pppp1ppp/8/4p3/4P2q/5N2/PPPP1PPP/RNBQKB1R w KQkq - 2 3",
        4,
    );
    let t2 = mv == "f3h4";
    println!(
        "  [{}] выигрыш ферзя: найден {}",
        if t2 { "OK " } else { "ОШИБКА" },
        mv
    );
    all_ok &= t2;

    // не оставлять мат в один ход: у чёрных ладья грозит Re1#, белые обязаны что-то сделать
    let fen_bk = "4r1k1/5ppp/8/8/8/8/5PPP/R5K1 w - - 0 1";
    let (mv, _) = best_move(fen_bk, 5);
    let t3 = {
        let b = Board::from_fen(fen_bk).unwrap();
        let l = b.legal_moves();
        let mut ok = false;
        for i in 0..l.n {
            if l.m[i].to_uci() == mv {
                let nb = b.make(l.m[i]);
                let r = nb.legal_moves();
                let mut mated = false;
                for j in 0..r.n {
                    let n2 = nb.make(r.m[j]);
                    if n2.in_check() && n2.legal_moves().n == 0 {
                        mated = true;
                    }
                }
                ok = !mated;
            }
        }
        ok
    };
    println!(
        "  [{}] не оставляет мат в один ход: сыграно {}",
        if t3 { "OK " } else { "ОШИБКА" },
        mv
    );
    all_ok &= t3;

    // Тактические цепочки остаются регрессиями после удаления рекурсивного доказателя.
    // Предсказанная линия не даёт права объявлять доказанный мат в исходной позиции.
    let fen_sac = "5r1k/6pp/7N/8/8/1Q6/8/6K1 w - - 0 1"; // 1.Qg8+! Rxg8 2.Nf7#
    let fen_nosac = "5r1k/6pp/8/8/8/1Q6/8/6K1 w - - 0 1"; // то же без коня: мата нет
    let fen_quiet = "6k1/5ppp/4n3/2N2RQ1/8/6K1/8/8 w - - 0 1"; // 1.Nxe6! (не шах), любой ответ -> мат
    let fen_def = "5r1k/6pp/7N/8/8/1Q6/8/6K1 b - - 0 1"; // 14 из 16 ходов чёрных проигрывают шахами

    let r = full_search(fen_sac, 5);
    let pv_txt: Vec<String> = r.lines[0].pv.iter().map(|m| m.to_uci()).collect();
    let t4 = r.best.to_uci() == "b3g8"
        && r.score.abs() < MATE_BOUND
        && pv_txt.join(" ") == "b3g8 f8g8 h6f7";
    println!(
        "  [{}] жертва ферзя с матом в 2: найдено {} (оценка {}), цепочка: {}",
        if t4 { "OK " } else { "ОШИБКА" },
        r.best.to_uci(),
        r.score,
        pv_txt.join(" ")
    );
    all_ok &= t4;

    let r = full_search(fen_nosac, 5);
    let t5 = r.score.abs() < MATE_BOUND;
    println!(
        "  [{}] без поддержки коня мат не заявляется (оценка {})",
        if t5 { "OK " } else { "ОШИБКА" },
        r.score
    );
    all_ok &= t5;

    let r = full_search(fen_quiet, 5);
    let t6 = r.best.to_uci() == "c5e6" && r.score.abs() < MATE_BOUND;
    println!(
        "  [{}] тихий первый ход, после которого любой ответ проигрывает: {} (оценка {})",
        if t6 { "OK " } else { "ОШИБКА" },
        r.best.to_uci(),
        r.score
    );
    all_ok &= t6;

    let r = full_search(fen_def, 5);
    // Два безопасных ответа: дать шах ладьёй на f1 либо забрать коня h6.
    let t7 = matches!(r.best.to_uci().as_str(), "f8f1" | "g7h6");
    println!(
        "  [{}] защита: выбран {}, после него у белых нет мата шахами за 2 хода",
        if t7 { "OK " } else { "ОШИБКА" },
        r.best.to_uci()
    );
    all_ok &= t7;

    let mut t8 = true;
    for fen in [fen_sac, fen_nosac, fen_quiet, fen_def] {
        let r = full_search(fen, 5);
        let mut b = Board::from_fen(fen).unwrap();
        for mv in &r.lines[0].pv {
            let legal = b.legal_moves();
            if !legal.m[..legal.n].contains(mv) {
                t8 = false;
                break;
            }
            b = b.make(*mv);
        }
    }
    println!(
        "  [{}] все ходы предсказанных тактических цепочек легальны",
        if t8 { "OK " } else { "ОШИБКА" }
    );
    all_ok &= t8;

    // Оценка тактических позиций симметрична.
    let mut fsym_ok = true;
    for f in [fen_sac, fen_nosac, fen_quiet, fen_def].iter() {
        let b = Board::from_fen(f).unwrap();
        let a = settle_value(&b, 100);
        let m = settle_value(&b.mirrored(), 100);
        if (a - m).abs() > 2 {
            fsym_ok = false;
        }
    }
    println!(
        "  [{}] симметрия оценки тактических позиций",
        if fsym_ok { "OK " } else { "ОШИБКА" }
    );
    all_ok &= fsym_ok;

    // доведение выигрыша: перевес огромный, но мата в 1 ход нет — движок должен строить сетку,
    // а не гонять фигуры (защитник ходит первым допустимым ходом)
    let conv = [
        "1k1K4/7P/2Q2Q2/1P6/8/8/8/8 w - - 0 1",
        "8/8/8/5QK1/Q1R1B3/8/8/6k1 w - - 0 1",
        "4Q3/P3P2k/2R5/3K4/8/8/8/8 w - - 0 1",
        "8/8/8/8/3k4/8/1Q6/K1R5 w - - 0 1",
    ];
    let mut conv_ok = true;
    let mut worst = 0;
    for f in conv.iter() {
        let mut b = Board::from_fen(f).unwrap();
        let mut hist: Vec<u64> = vec![b.hash];
        let mut mated_at = -1;
        for mv_no in 0..12 {
            let stop = Arc::new(AtomicBool::new(false));
            let cfg = EngineConfig::default();
            let mut s = Searcher::new(&cfg, stop);
            let limits = SearchLimits {
                max_depth: 5,
                time_ms: 0,
                variants: 1,
            };
            let res = s.think(&b, &hist, &limits, Move::NONE, Report::Quiet);
            b = b.make(res.best);
            hist.push(b.hash);
            let r = b.legal_moves();
            if r.n == 0 {
                if b.in_check() {
                    mated_at = mv_no + 1;
                }
                break;
            }
            b = b.make(r.m[0]);
            hist.push(b.hash);
        }
        if mated_at < 0 {
            conv_ok = false;
        } else if mated_at > worst {
            worst = mated_at;
        }
    }
    println!(
        "  [{}] доведение выигрыша: 4 позиции с огромным перевесом заматованы (дольше всего {} ходов)",
        if conv_ok { "OK " } else { "ОШИБКА" },
        worst
    );
    all_ok &= conv_ok;

    println!("4) Ядра, кэш и настройки:");
    let mut cfg_ok = true;
    let mut c = EngineConfig::default();
    c.set_threads(0);
    cfg_ok &= c.threads == THREADS_MIN;
    c.set_threads(100000);
    cfg_ok &= c.threads == THREADS_MAX;
    c.set_cache_mb(-5);
    cfg_ok &= c.cache_mb == CACHE_MIN_MB;
    c.set_cache_mb(1 << 40);
    cfg_ok &= c.cache_mb == CACHE_MAX_MB;
    c.set_threat(1000);
    cfg_ok &= c.threat_pct == THREAT_MAX;
    c.set_chain(-1);
    cfg_ok &= c.chain_pct == CHAIN_MIN;
    println!(
        "  [{}] границы настроек",
        if cfg_ok { "OK " } else { "ОШИБКА" }
    );
    all_ok &= cfg_ok;

    let cache = WaveCache::new(1);
    cache.put(123456789, -77);
    cache.put(42, 31000);
    let mut cache_ok = cache.get(123456789) == Some(-77) && cache.get(42) == Some(31000);
    cache_ok &= cache.get(987654321).is_none();
    cache.clear();
    cache_ok &= cache.get(123456789).is_none();
    let off = WaveCache::new(0);
    off.put(5, 5);
    cache_ok &= off.get(5).is_none() && !off.is_enabled();
    println!(
        "  [{}] кэш: запись, чтение, очистка, выключение",
        if cache_ok { "OK " } else { "ОШИБКА" }
    );
    all_ok &= cache_ok;

    // ядра и кэш меняют только скорость, но не выбор хода
    let det_fens = [
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
    ];
    let mut det_ok = true;
    for f in det_fens.iter() {
        let a = best_move_with(f, 10, 1, 0);
        let b = best_move_with(f, 10, 4, 16);
        if a != b {
            det_ok = false;
        }
    }
    println!(
        "  [{}] результат не зависит от числа ядер и кэша",
        if det_ok { "OK " } else { "ОШИБКА" }
    );
    all_ok &= det_ok;

    println!("5) Запись ходов (SAN) и разбор ввода:");
    let kb =
        Board::from_fen("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1")
            .unwrap();
    let legal = kb.legal_moves();
    let mut san_ok = true;
    for i in 0..legal.n {
        let m = legal.m[i];
        let san = notation::move_to_san(&kb, m);
        match notation::parse_move(&kb, &san) {
            Some(p) => {
                if p != m {
                    san_ok = false;
                }
            }
            None => san_ok = false,
        }
    }
    println!(
        "  [{}] круговая проверка SAN для {} ходов",
        if san_ok { "OK " } else { "ОШИБКА" },
        legal.n
    );
    all_ok &= san_ok;

    println!();
    if all_ok {
        println!("ВСЕ ПРОВЕРКИ ПРОЙДЕНЫ");
    } else {
        println!("ЕСТЬ ОШИБКИ");
    }
    all_ok
}
