//! Консольная игра против движка (на русском языке).

use crate::board::*;
use crate::engine::*;
use crate::geometry::*;
use crate::notation;
use crate::search::*;
use crate::wave;
use std::io::{self, Write};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

struct Settings {
    think_ms: u64,
    depth: i32,
    variants: usize,
}

fn read_prompt(prompt: &str) -> Option<String> {
    print!("{}", prompt);
    let _ = io::stdout().flush();
    let mut s = String::new();
    match io::stdin().read_line(&mut s) {
        Ok(0) => None,
        Ok(_) => Some(s.trim().to_string()),
        Err(_) => None,
    }
}

fn print_banner() {
    println!("==============================================================");
    println!("  WaveChess — шахматный движок на волновой математике");
    println!("  Без битбордов. Без обучения. Только поле волн и математика.");
    println!("==============================================================");
    println!("Каждая фигура излучает волну по своим векторам; сумма волн —");
    println!("поле давления и энергии. Продолжения — прогнозы модели,");
    println!("а не доказательство выигрыша. Введите help для команд.");
    println!();
}

fn print_help() {
    println!("Ход вводится так: e2e4, e7e8q (координаты) или Nf3, exd5, O-O (SAN).");
    println!("Команды:");
    println!("  help            эта справка");
    println!("  moves           список допустимых ходов");
    println!("  hint            совет: движок показывает варианты, но не ходит");
    println!("  go              движок делает ход за сторону, которая ходит сейчас");
    println!("  side            поменяться сторонами");
    println!("  undo            взять ход назад (вместе с ответом движка)");
    println!("  wave            карта волнового поля и разложение энергии");
    println!("  memory          память движка: партия, баланс по ходам, план");
    println!("  save FILE       сохранить партию и планы в файл");
    println!("  load FILE       восстановить сохранённую партию и планы");
    println!("  variety N       окно разнообразия дебюта, сотые пешки (0..100)");
    println!("  seed N          воспроизводимое разнообразие (0 — новый seed при запуске)");
    println!("  width N         число основных корневых волн (1..32)");
    println!("  time N          предел времени на ход, секунд (например: time 5)");
    println!(
        "  depth N         максимальная длина волновой цепочки, полуходов (1..{})",
        crate::search::MAX_DEPTH
    );
    println!("  variants N      сколько вариантов цепочек показывать (1..6)");
    println!("  settings        показать настройки ядер, кэша и волновой модели");
    println!("  threads N       число ядер (потоков) для расчёта");
    println!("  cache N         размер кэша волновых оценок, МБ (0 — выключить)");
    println!("  clearcache      очистить кэш");
    println!("  threat N        вес волны угрозы, % (0..300, по умолчанию 130)");
    println!("  chain N         вес прогноза, % (0..100, по умолчанию 40)");
    println!("  fen             показать позицию в FEN");
    println!("  setfen FEN      загрузить позицию");
    println!("  perft N         проверка правил: подсчёт узлов на глубину N");
    println!("  new             новая партия");
    println!("  quit            выход");
}

fn choose_side() -> Option<i8> {
    println!(
        "За кого играете?  w — белые, b — чёрные, e — смотреть, как движок играет сам с собой"
    );
    let s = read_prompt("> ")?;
    let l = s.to_lowercase();
    Some(match l.as_str() {
        "b" | "ч" | "black" | "чёрные" | "черные" => -1,
        "e" | "с" | "engine" => 0,
        _ => 1,
    })
}

fn print_board(eng: &Engine, human: i8) {
    let b = &eng.board;
    let flip = human == -1;
    println!();
    if flip {
        println!("     h g f e d c b a");
    } else {
        println!("     a b c d e f g h");
    }
    println!("   +-----------------+");
    for i in 0..8i8 {
        let r = if flip { i } else { 7 - i };
        let mut row = format!(" {} | ", r + 1);
        for j in 0..8i8 {
            let f = if flip { 7 - j } else { j };
            row.push(piece_char(b.sq[sq_of(f, r)]));
            row.push(' ');
        }
        row.push_str(&format!("| {}", r + 1));
        println!("{}", row);
    }
    println!("   +-----------------+");
    if flip {
        println!("     h g f e d c b a");
    } else {
        println!("     a b c d e f g h");
    }
    if let Some(r) = eng.history.last() {
        println!("Последний ход: {} ({})", r.san, r.mv.to_uci());
    }
    let (_, e) = wave::analyze(b);
    let cp = e.total.round() as i32;
    let sh = wave::white_share(cp);
    println!(
        "Волна: белые {:.0}% | чёрные {:.0}%   баланс {:+.2} — {}",
        sh * 100.0,
        (1.0 - sh) * 100.0,
        cp as f32 / 100.0,
        wave::verdict(cp)
    );
    let who = if b.side == 1 {
        "белых"
    } else {
        "чёрных"
    };
    if b.in_check() {
        println!("Ход {} (ШАХ), ход № {}", who, b.fullmove);
    } else {
        println!("Ход {}, ход № {}", who, b.fullmove);
    }
}

fn status_text(s: Status) -> String {
    match s {
        Status::Ongoing => String::new(),
        Status::Checkmate(w) => {
            format!("МАТ. Победили {}.", if w == 1 { "белые" } else { "чёрные" })
        }
        Status::Stalemate => "ПАТ. Ничья.".to_string(),
        Status::DrawFifty => "Ничья по правилу 50 ходов.".to_string(),
        Status::DrawRepetition => "Ничья: троекратное повторение позиции.".to_string(),
        Status::DrawMaterial => "Ничья: недостаточно материала для мата.".to_string(),
    }
}

fn print_analysis(before: &Board, res: &SearchResult) {
    println!(
        "  Длина цепочки {}, осевших позиций {}, время {:.2} с",
        res.depth,
        res.nodes,
        res.elapsed_ms as f32 / 1000.0
    );
    for (i, l) in res.lines.iter().enumerate() {
        let end = notation::apply_pv(before, &l.pv);
        let cp = wave::evaluate(&end);
        let sh = wave::white_share(cp);
        println!(
            "  Вариант {}: [{}]  {}",
            i + 1,
            notation::format_score(l.score, before.side),
            notation::pv_to_san(before, &l.pv, 10)
        );
        println!(
            "             волна в конце цепочки: белые {:.0}% / чёрные {:.0}%",
            sh * 100.0,
            (1.0 - sh) * 100.0
        );
        println!(
            "             ближайшая энергия {:+.2}, уверенность модели {:.1}%, траекторий {}",
            l.local_energy as f32 / 100.0,
            l.confidence * 100.0,
            l.replies
        );
    }
}

fn engine_move(eng: &mut Engine, st: &Settings) {
    let before = eng.board;
    println!("Волна думает ({:.1} с)...", st.think_ms as f32 / 1000.0);
    let limits = SearchLimits {
        max_depth: st.depth,
        time_ms: st.think_ms,
        variants: st.variants,
    };
    let res = eng.think(&limits, Report::Quiet, true);
    if res.best.is_none() {
        println!("У движка нет ходов.");
        return;
    }
    print_analysis(&before, &res);
    let san = notation::move_to_san(&before, res.best);
    eng.play(res.best, true);
    println!("Ход движка: {}  ({})", san, res.best.to_uci());
    if !eng.plan.is_empty() {
        println!(
            "Ожидаемое продолжение: {}",
            notation::pv_to_san(&eng.board, &eng.plan, 8)
        );
    }
}

#[cfg(windows)]
fn pause_on_exit() {
    let _ = read_prompt("Нажмите Enter, чтобы закрыть окно...");
}

#[cfg(not(windows))]
fn pause_on_exit() {}

pub fn run() {
    let stop = Arc::new(AtomicBool::new(false));
    let mut eng = Engine::new(stop);
    let mut st = Settings {
        think_ms: 3000,
        depth: crate::search::DEFAULT_DEPTH,
        variants: 3,
    };
    print_banner();
    let mut human: i8 = match choose_side() {
        Some(h) => h,
        None => return,
    };
    loop {
        print_board(&eng, human);
        let status = eng.status();
        let over = status != Status::Ongoing;
        if over {
            println!("{}", status_text(status));
        }
        let engine_turn = !over && (human == 0 || eng.board.side != human);
        if engine_turn {
            engine_move(&mut eng, &st);
            continue;
        }

        let mut state_changed = false;
        while !state_changed {
            let prompt = if over {
                "Партия окончена (new — заново, undo — назад, quit)> "
            } else {
                "Ваш ход> "
            };
            let line = match read_prompt(prompt) {
                Some(l) => l,
                None => return,
            };
            if line.is_empty() {
                continue;
            }
            let words: Vec<&str> = line.split_whitespace().collect();
            let cmd = words[0].to_lowercase();
            match cmd.as_str() {
                "help" | "?" | "помощь" => print_help(),
                "quit" | "exit" | "q" | "выход" => {
                    println!("До встречи!");
                    pause_on_exit();
                    return;
                }
                "new" | "новая" => {
                    eng.new_game();
                    human = match choose_side() {
                        Some(h) => h,
                        None => return,
                    };
                    state_changed = true;
                }
                "undo" | "back" | "назад" => {
                    if !eng.undo() {
                        println!("Нечего возвращать.");
                    } else {
                        if human != 0 && eng.board.side != human {
                            eng.undo();
                        }
                        state_changed = true;
                    }
                }
                "wave" | "волна" | "eval" | "оценка" => {
                    println!("{}", wave::wave_map(&eng.board));
                    let (_, e) = wave::analyze(&eng.board);
                    println!("Энергия поля (в пешках, с точки зрения белых):");
                    println!("{}", e.describe());
                }
                "moves" | "ходы" => {
                    let legal = eng.board.legal_moves();
                    let mut out = String::new();
                    for i in 0..legal.n {
                        out.push_str(&notation::move_to_san(&eng.board, legal.m[i]));
                        out.push(' ');
                    }
                    println!("Допустимые ходы ({}): {}", legal.n, out);
                }
                "hint" | "совет" => {
                    if over {
                        println!("Партия окончена.");
                    } else {
                        let before = eng.board;
                        println!("Анализ волн ({:.1} с)...", st.think_ms as f32 / 1000.0);
                        let limits = SearchLimits {
                            max_depth: st.depth,
                            time_ms: st.think_ms,
                            variants: st.variants.max(3),
                        };
                        let res = eng.think(&limits, Report::Quiet, false);
                        print_analysis(&before, &res);
                    }
                }
                "go" | "ход" => {
                    if over {
                        println!("Партия окончена.");
                    } else {
                        human = -eng.board.side;
                        state_changed = true;
                    }
                }
                "side" | "сторона" => {
                    if human != 0 {
                        human = -human;
                        state_changed = true;
                    } else {
                        println!("Движок играет сам с собой.");
                    }
                }
                "time" | "время" => {
                    let v = words
                        .get(1)
                        .and_then(|x| x.replace(',', ".").parse::<f32>().ok());
                    match v {
                        Some(sec) if sec >= 0.05 && sec <= 3600.0 => {
                            st.think_ms = (sec * 1000.0) as u64;
                            println!("Время на ход: {:.2} с", sec);
                        }
                        _ => println!("Пример: time 5"),
                    }
                }
                "depth" | "глубина" => {
                    let v = words.get(1).and_then(|x| x.parse::<i32>().ok());
                    match v {
                        Some(d) if d >= 1 && d <= crate::search::MAX_DEPTH => {
                            st.depth = d;
                            println!("Максимальная длина цепочки: {}", d);
                        }
                        _ => println!("Пример: depth 8"),
                    }
                }
                "variants" | "варианты" => {
                    let v = words.get(1).and_then(|x| x.parse::<usize>().ok());
                    match v {
                        Some(n) if n >= 1 && n <= 6 => {
                            st.variants = n;
                            println!("Вариантов цепочек: {}", n);
                        }
                        _ => println!("Пример: variants 3"),
                    }
                }
                "settings" | "настройки" => {
                    println!("Настройки движка:");
                    println!("{}", eng.config().describe());
                    print!("{}", eng.searcher.cache_report());
                }
                "threads" | "ядра" | "потоки" => {
                    match words.get(1).and_then(|x| x.parse::<i64>().ok()) {
                        Some(v) => {
                            let mut c = eng.config();
                            c.set_threads(v);
                            eng.set_config(&c);
                            println!("Ядер (потоков): {}", eng.config().threads);
                        }
                        None => println!("Пример: threads 4"),
                    }
                }
                "cache" | "hash" | "кэш" | "кеш" => {
                    match words.get(1).and_then(|x| x.parse::<i64>().ok()) {
                        Some(v) => {
                            let mut c = eng.config();
                            c.set_cache_mb(v);
                            eng.set_config(&c);
                            if eng.config().cache_mb == 0 {
                                println!("Кэш выключен.");
                            } else {
                                println!("Кэш: {} МБ", eng.config().cache_mb);
                            }
                        }
                        None => println!("Пример: cache 64   (0 — выключить)"),
                    }
                }
                "clearcache" | "clearhash" => {
                    eng.searcher.clear();
                    println!("Кэш очищен.");
                }
                "threat" | "угроза" => {
                    match words.get(1).and_then(|x| x.parse::<i64>().ok()) {
                        Some(v) => {
                            let mut c = eng.config();
                            c.set_threat(v);
                            eng.set_config(&c);
                            println!("Вес волны угрозы: {} %", eng.config().threat_pct);
                        }
                        None => println!("Пример: threat 100"),
                    }
                }
                "chain" | "цепочка" => {
                    match words.get(1).and_then(|x| x.parse::<i64>().ok()) {
                        Some(v) => {
                            let mut c = eng.config();
                            c.set_chain(v);
                            eng.set_config(&c);
                            println!("Вес конца цепочки: {} %", eng.config().chain_pct);
                        }
                        None => println!("Пример: chain 40"),
                    }
                }
                "memory" | "память" | "history" => {
                    println!("{}", eng.memory_report());
                }
                "save" => {
                    let path = line.split_once(char::is_whitespace).map(|(_, p)| p.trim());
                    match path.filter(|p| !p.is_empty()) {
                        Some(p) => match std::fs::write(p, eng.save_game()) {
                            Ok(()) => println!("Партия и планы сохранены: {}", p),
                            Err(e) => println!("Не удалось сохранить: {}", e),
                        },
                        None => println!("Пример: save game.wave"),
                    }
                }
                "load" => {
                    let path = line.split_once(char::is_whitespace).map(|(_, p)| p.trim());
                    match path.filter(|p| !p.is_empty()) {
                        Some(p) => match std::fs::read_to_string(p)
                            .map_err(|e| e.to_string())
                            .and_then(|text| eng.load_game(&text))
                        {
                            Ok(()) => {
                                println!("Партия восстановлена: {}", p);
                                state_changed = true;
                            }
                            Err(e) => println!("Не удалось восстановить: {}", e),
                        },
                        None => println!("Пример: load game.wave"),
                    }
                }
                "variety" | "seed" | "width" => {
                    if let Some(v) = words.get(1).and_then(|x| x.parse::<i64>().ok()) {
                        let mut c = eng.config();
                        match cmd.as_str() {
                            "variety" => c.opening_variety = v.clamp(0, 100) as i32,
                            "width" => c.wave_width = v.clamp(1, 32) as usize,
                            _ => c.random_seed = v.clamp(0, 2147483647) as u64,
                        }
                        eng.set_config(&c);
                        println!(
                            "Ширина волн: {}, разнообразие: {}, seed: {}",
                            c.wave_width, c.opening_variety, c.random_seed
                        );
                    } else {
                        println!("Примеры: variety 20, seed 42, width 6");
                    }
                }
                "fen" => {
                    println!("{}", eng.board.to_fen());
                }
                "setfen" => {
                    if words.len() < 5 {
                        println!("Пример: setfen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1");
                    } else {
                        let fen = words[1..].join(" ");
                        match Board::from_fen(&fen) {
                            Ok(b) => {
                                eng.set_board(b);
                                state_changed = true;
                            }
                            Err(e) => println!("Ошибка: {}", e),
                        }
                    }
                }
                "perft" => {
                    let d = words
                        .get(1)
                        .and_then(|x| x.parse::<u32>().ok())
                        .unwrap_or(4);
                    let t0 = Instant::now();
                    let n = eng.board.perft(d);
                    println!(
                        "perft({}) = {}   ({:.2} с)",
                        d,
                        n,
                        t0.elapsed().as_secs_f32()
                    );
                }
                _ => {
                    if over {
                        println!("Партия окончена. new — новая партия, undo — назад.");
                    } else {
                        match notation::parse_move(&eng.board, &line) {
                            Some(mv) => {
                                eng.play(mv, human == 0);
                                state_changed = true;
                            }
                            None => println!(
                                "Не понял «{}». Примеры: e2e4, Nf3, O-O. Команда moves покажет все ходы.",
                                line
                            ),
                        }
                    }
                }
            }
        }
    }
}
