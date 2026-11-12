//! Движок: позиция + память партии + план (цепочка ходов, построенная наперёд).

use crate::board::*;
use crate::notation;
use crate::search::*;
use crate::settings::EngineConfig;
use crate::wave;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct MoveRecord {
    pub mv: Move,
    pub san: String,
    pub by: i8,             // кто ходил: 1 белые, -1 чёрные
    pub balance_after: i32, // волновой баланс после хода (в сотых пешки, с точки зрения белых)
    pub predicted: bool,    // был ли этот ход предсказан планом
    pub prediction_attempted: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ongoing,
    Checkmate(i8), // победитель
    Stalemate,
    DrawFifty,
    DrawRepetition,
    DrawMaterial,
}

pub struct Engine {
    pub board: Board,
    pub boards: Vec<Board>,
    pub hashes: Vec<u64>,
    pub history: Vec<MoveRecord>,
    pub plan: Vec<Move>,
    plans: Vec<Vec<Move>>,
    session_seed: u64,
    game_number: u64,
    pub searcher: Searcher,
    pub predicted_hits: u32,
    pub predicted_total: u32,
}

impl Engine {
    pub fn new(stop: Arc<AtomicBool>) -> Engine {
        let b = Board::startpos();
        Engine {
            board: b,
            boards: Vec::new(),
            hashes: vec![b.hash],
            history: Vec::new(),
            plan: Vec::new(),
            plans: Vec::new(),
            session_seed: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            game_number: 0,
            searcher: Searcher::new(&EngineConfig::default(), stop),
            predicted_hits: 0,
            predicted_total: 0,
        }
    }

    pub fn new_game(&mut self) {
        self.game_number = self.game_number.wrapping_add(1);
        self.set_board(Board::startpos());
        self.searcher.clear();
        self.predicted_hits = 0;
        self.predicted_total = 0;
    }

    /// Применить настройки ядер, кэша и волновой модели.
    pub fn set_config(&mut self, cfg: &EngineConfig) {
        self.searcher.apply_config(cfg);
    }

    pub fn config(&self) -> EngineConfig {
        self.searcher.config()
    }

    pub fn set_board(&mut self, b: Board) {
        self.board = b;
        self.boards.clear();
        self.hashes.clear();
        self.hashes.push(b.hash);
        self.history.clear();
        self.plan.clear();
        self.plans.clear();
        self.predicted_hits = 0;
        self.predicted_total = 0;
    }

    /// Сыграть ход. `by_engine` — ход сделал сам движок (иначе это ответ соперника).
    pub fn play(&mut self, mv: Move, by_engine: bool) {
        let san = notation::move_to_san(&self.board, mv);
        let mut predicted = false;
        if self.plans.is_empty() && !self.plan.is_empty() {
            self.plans.push(self.plan.clone());
        }
        let prediction_attempted = !by_engine && !self.plans.is_empty();
        if prediction_attempted {
            self.predicted_total += 1;
            if self.plans.iter().any(|p| p.first() == Some(&mv)) {
                predicted = true;
                self.predicted_hits += 1;
            }
        }
        self.plans.retain(|p| p.first() == Some(&mv));
        for p in &mut self.plans {
            p.remove(0);
        }
        self.plans.retain(|p| !p.is_empty());
        self.plan = self.plans.first().cloned().unwrap_or_default();
        let by = self.board.side;
        self.boards.push(self.board);
        self.board = self.board.make(mv);
        self.hashes.push(self.board.hash);
        self.history.push(MoveRecord {
            mv,
            san,
            by,
            balance_after: wave::evaluate(&self.board),
            predicted,
            prediction_attempted,
        });
    }

    pub fn undo(&mut self) -> bool {
        match self.boards.pop() {
            Some(b) => {
                self.board = b;
                self.hashes.pop();
                self.history.pop();
                self.plan.clear();
                self.plans.clear();
                // После отмены статистика относится только к оставшимся ходам.
                self.predicted_hits = self.history.iter().filter(|r| r.predicted).count() as u32;
                self.predicted_total = self
                    .history
                    .iter()
                    .filter(|r| r.prediction_attempted)
                    .count() as u32;
                true
            }
            None => false,
        }
    }

    pub fn think(
        &mut self,
        limits: &SearchLimits,
        report: Report,
        store_plan: bool,
    ) -> SearchResult {
        let hint = if self.plan.is_empty() {
            Move::NONE
        } else {
            self.plan[0]
        };
        let variety = store_plan
            && self.config().opening_variety > 0
            && self.board.fullmove <= 6
            && !self.board.in_check();
        let expanded = SearchLimits {
            max_depth: limits.max_depth,
            time_ms: limits.time_ms,
            variants: if variety { 256 } else { limits.variants },
        };
        self.searcher.report_variants = limits.variants.max(1);
        let mut res = self
            .searcher
            .think(&self.board, &self.hashes, &expanded, hint, report);
        if variety && res.depth > 0 && res.score.abs() < MATE_BOUND && !res.lines.is_empty() {
            let cfg = self.config();
            let seed = if cfg.random_seed == 0 {
                self.session_seed
            } else {
                cfg.random_seed
            };
            let key = mix(seed ^ self.board.hash ^ mix(self.game_number));
            let peak = res.lines[0].score;
            let candidates: Vec<usize> = res
                .lines
                .iter()
                .enumerate()
                .filter(|(_, l)| {
                    l.score >= peak - cfg.opening_variety && l.score.abs() < MATE_BOUND
                })
                .map(|(i, _)| i)
                .collect();
            // Нормированные амплитуды только внутри узкого энергетического окна.
            let tau = (cfg.opening_variety as f64 / 2.0).max(1.0);
            let amplitudes: Vec<f64> = candidates
                .iter()
                .map(|&i| ((res.lines[i].score - peak) as f64 / tau).exp())
                .collect();
            let mut ticket =
                ((key >> 11) as f64 / (1u64 << 53) as f64) * amplitudes.iter().sum::<f64>();
            let mut selected = candidates[0];
            for (&i, a) in candidates.iter().zip(amplitudes) {
                selected = i;
                ticket -= a;
                if ticket <= 0.0 {
                    break;
                }
            }
            res.lines.swap(0, selected);
            res.best = res.lines[0].pv[0];
            res.score = res.lines[0].score;
            res.depth = res.lines[0].depth;
        }
        res.lines.truncate(limits.variants.max(1));
        if variety && report == Report::Uci {
            for (i, l) in res.lines.iter().enumerate() {
                let pv = l.pv.iter().map(Move::to_uci).collect::<Vec<_>>().join(" ");
                let score = if l.score.abs() >= MATE_BOUND {
                    format!(
                        "mate {}",
                        ((MATE - l.score.abs() + 1) / 2) * l.score.signum()
                    )
                } else {
                    format!("cp {}", l.score)
                };
                println!(
                    "info depth {} multipv {} score {} nodes {} time {} pv {}",
                    l.depth,
                    i + 1,
                    score,
                    res.nodes,
                    res.elapsed_ms,
                    pv
                );
            }
        }
        if store_plan {
            if res.lines.is_empty() {
                self.plan.clear();
            } else {
                self.plan = res.lines[0].pv.clone();
            }
            self.plans = res
                .continuations
                .into_iter()
                .filter(|p| p.first() == Some(&res.best))
                .collect();
            // Главный прогноз идёт первым, остальные не теряются при ином ответе.
            self.plans.retain(|p| p != &self.plan);
            if !self.plan.is_empty() {
                self.plans.insert(0, self.plan.clone());
            }
            res.continuations = self.plans.clone();
        }
        if report == Report::Uci {
            if let Some(l) = res.lines.first() {
                println!("info string wave local_cp {} forecast_cp {} confidence {:.3} trajectories {} plies {}",
                    l.local_energy, l.score, l.confidence, l.replies, l.depth);
            }
        }
        res
    }

    /// Оболочки иногда присылают только новый FEN. Восстанавливаем один или
    /// два легальных хода от известной позиции, сохраняя историю повторений.
    /// Для несвязанного FEN явно начинаем новый контекст.
    pub fn sync_board(&mut self, b: Board, engine_side: i8) {
        fn same(a: &Board, b: &Board) -> bool {
            a.sq == b.sq
                && a.side == b.side
                && a.castle == b.castle
                && a.hash == b.hash
                && a.halfmove == b.halfmove
                && a.fullmove == b.fullmove
        }
        if same(&self.board, &b) {
            return;
        }
        let ply = |p: &Board| 2 * p.fullmove as i32 + if p.side == -1 { 1 } else { 0 };
        let delta = ply(&b) - ply(&self.board);
        if (1..=2).contains(&delta) {
            let legal = self.board.legal_moves();
            for &first in &legal.m[..legal.n] {
                let next = self.board.make(first);
                if delta == 1 && same(&next, &b) {
                    self.play(first, self.board.side == engine_side);
                    return;
                }
                if delta == 2 {
                    let replies = next.legal_moves();
                    for &second in &replies.m[..replies.n] {
                        if same(&next.make(second), &b) {
                            self.play(first, self.board.side == engine_side);
                            self.play(second, self.board.side == engine_side);
                            return;
                        }
                    }
                }
            }
        }
        self.set_board(b);
    }

    /// Переносимый журнал. Никаких весов обучения или результатов поиска в кэше.
    pub fn save_game(&self) -> String {
        let start = self.boards.first().unwrap_or(&self.board);
        let mut text = format!(
            "WaveChess-memory-v1\nfen {}\nmoves {}\nseed {} {}\n",
            start.to_fen(),
            self.history
                .iter()
                .map(|r| r.mv.to_uci())
                .collect::<Vec<_>>()
                .join(" "),
            if self.config().random_seed == 0 {
                self.session_seed
            } else {
                self.config().random_seed
            },
            self.game_number
        );
        let flags: String = self
            .history
            .iter()
            .map(|r| {
                if r.predicted {
                    '2'
                } else if r.prediction_attempted {
                    '1'
                } else {
                    '0'
                }
            })
            .collect();
        text.push_str(&format!("flags {}\n", flags));
        for plan in &self.plans {
            text.push_str(&format!(
                "plan {}\n",
                plan.iter().map(Move::to_uci).collect::<Vec<_>>().join(" ")
            ));
        }
        text
    }

    pub fn load_game(&mut self, text: &str) -> Result<(), String> {
        if text.len() > 1_000_000 {
            return Err("Слишком большой журнал".into());
        }
        let mut lines = text.lines();
        if lines.next() != Some("WaveChess-memory-v1") {
            return Err("Неверный формат журнала".into());
        }
        let fen = lines
            .next()
            .and_then(|s| s.strip_prefix("fen "))
            .ok_or("Нет начальной позиции")?;
        let moves = lines
            .next()
            .and_then(|s| s.strip_prefix("moves "))
            .ok_or("Нет списка ходов")?;
        let mut next = Engine::new(Arc::new(AtomicBool::new(false)));
        next.set_board(Board::from_fen(fen)?);
        for word in moves.split_whitespace() {
            let mv = notation::parse_move(&next.board, word).ok_or("Нелегальный ход в журнале")?;
            next.play(mv, false);
        }
        for line in lines {
            if let Some(values) = line.strip_prefix("seed ") {
                let mut v = values.split_whitespace();
                next.session_seed = v
                    .next()
                    .and_then(|x| x.parse().ok())
                    .ok_or("Неверный seed")?;
                next.game_number = v
                    .next()
                    .and_then(|x| x.parse().ok())
                    .ok_or("Неверный номер партии")?;
            } else if let Some(values) = line.strip_prefix("flags ") {
                if values.len() != next.history.len()
                    || !values.bytes().all(|b| (b'0'..=b'2').contains(&b))
                {
                    return Err("Неверные отметки истории".into());
                }
                for (r, flag) in next.history.iter_mut().zip(values.bytes()) {
                    r.predicted = flag == b'2';
                    r.prediction_attempted = flag != b'0';
                }
            } else if let Some(values) = line.strip_prefix("plan ") {
                let mut b = next.board;
                let mut plan = Vec::new();
                for word in values.split_whitespace() {
                    let mv = notation::parse_move(&b, word).ok_or("Нелегальный план в журнале")?;
                    b = b.make(mv);
                    plan.push(mv);
                }
                if !plan.is_empty() {
                    next.plans.push(plan);
                }
            } else {
                return Err("Неизвестная строка журнала".into());
            }
        }
        // До этой точки текущая партия не меняется даже при повреждённом файле.
        self.board = next.board;
        self.boards = next.boards;
        self.hashes = next.hashes;
        self.history = next.history;
        self.plans = next.plans;
        self.plan = self.plans.first().cloned().unwrap_or_default();
        self.session_seed = next.session_seed;
        self.game_number = next.game_number;
        self.predicted_hits = self.history.iter().filter(|r| r.predicted).count() as u32;
        self.predicted_total = self
            .history
            .iter()
            .filter(|r| r.prediction_attempted)
            .count() as u32;
        Ok(())
    }

    pub fn status(&self) -> Status {
        let legal = self.board.legal_moves();
        if legal.n == 0 {
            if self.board.in_check() {
                return Status::Checkmate(-self.board.side);
            }
            return Status::Stalemate;
        }
        if self.board.insufficient() {
            return Status::DrawMaterial;
        }
        if self.board.halfmove >= 100 {
            return Status::DrawFifty;
        }
        let cur = self.board.hash;
        let cnt = self.hashes.iter().filter(|&&h| h == cur).count();
        if cnt >= 3 {
            return Status::DrawRepetition;
        }
        Status::Ongoing
    }

    /// Отчёт о памяти движка: ходы партии, траектория волнового баланса, план.
    pub fn memory_report(&self) -> String {
        let mut s = String::new();
        s.push_str("Память движка\n");
        if self.history.is_empty() {
            s.push_str("  ходов в партии ещё не было\n");
        } else {
            s.push_str("  сыгранные ходы и волновой баланс после каждого:\n");
            let mut line = String::new();
            for (i, r) in self.history.iter().enumerate() {
                let who = if r.by == 1 { "б" } else { "ч" };
                let mark = if r.predicted { "*" } else { "" };
                line.push_str(&format!(
                    "{}.{}{} {}{} ({:+.2})  ",
                    i / 2 + 1,
                    if r.by == 1 { "" } else { ".." },
                    who,
                    r.san,
                    mark,
                    r.balance_after as f32 / 100.0
                ));
                if (i + 1) % 4 == 0 {
                    line.push('\n');
                    s.push_str("    ");
                    s.push_str(&line);
                    line.clear();
                }
            }
            if !line.is_empty() {
                s.push_str("    ");
                s.push_str(&line);
                s.push('\n');
            }
            s.push_str("  (* — ответ соперника совпал с предсказанием плана)\n");
        }
        s.push_str(&format!(
            "  предсказано ответов соперника: {} из {}\n",
            self.predicted_hits, self.predicted_total
        ));
        if self.plan.is_empty() {
            s.push_str("  текущий план: нет\n");
        } else {
            s.push_str(&format!(
                "  текущий план (цепочка наперёд): {}\n",
                notation::pv_to_san(&self.board, &self.plan, 12)
            ));
        }
        s.push_str(&format!(
            "  позиций в памяти партии: {}\n",
            self.hashes.len()
        ));
        s.push_str(&self.searcher.cache_report());
        s.push_str(&format!(
            "  сохранённых продолжений: {}\n",
            self.plans.len()
        ));
        s
    }
}

fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        let mut e = Engine::new(Arc::new(AtomicBool::new(false)));
        let mut cfg = e.config();
        cfg.threads = 1;
        cfg.cache_mb = 0;
        cfg.random_seed = 42;
        e.set_config(&cfg);
        e
    }

    fn play(e: &mut Engine, text: &str) {
        let mv = notation::parse_move(&e.board, text).unwrap();
        e.play(mv, false);
    }

    #[test]
    fn fen_refresh_preserves_history_repetition_and_same_position_plan() {
        let mut e = engine();
        for word in [
            "g1f3", "g8f6", "f3g1", "f6g8", "g1f3", "g8f6", "f3g1", "f6g8",
        ] {
            let mv = notation::parse_move(&e.board, word).unwrap();
            let next = e.board.make(mv);
            e.sync_board(next, 1);
        }
        assert!(matches!(e.status(), Status::DrawRepetition));
        assert_eq!(e.history.len(), 8);
        let mv = notation::parse_move(&e.board, "e2e4").unwrap();
        e.plan = vec![mv];
        e.sync_board(e.board, 1);
        assert_eq!(e.plan, vec![mv]);
    }

    #[test]
    fn fen_refresh_recovers_two_moves_and_resets_unrelated_position() {
        let mut e = engine();
        let mut actual = engine();
        play(&mut actual, "e2e4");
        play(&mut actual, "e7e5");
        e.sync_board(actual.board, 1);
        assert_eq!(e.history.len(), 2);
        assert_eq!(e.hashes, actual.hashes);
        e.sync_board(
            Board::from_fen("4k3/8/8/8/8/8/8/R3K3 w - - 0 1").unwrap(),
            1,
        );
        assert_eq!(e.history.len(), 0);
        assert_eq!(e.hashes.len(), 1);
    }

    #[test]
    fn alternative_reply_keeps_its_continuation() {
        let mut e = engine();
        play(&mut e, "e2e4");
        let line = |words: &[&str]| {
            let mut b = e.board;
            words
                .iter()
                .map(|word| {
                    let mv = notation::parse_move(&b, word).unwrap();
                    b = b.make(mv);
                    mv
                })
                .collect::<Vec<_>>()
        };
        e.plans = vec![line(&["e7e5", "g1f3"]), line(&["c7c5", "b1c3"])];
        e.plan = e.plans[0].clone();
        play(&mut e, "c7c5");
        assert_eq!(e.plan[0].to_uci(), "b1c3");
        assert_eq!(e.predicted_hits, 1);
        assert!(e.undo());
        assert_eq!(e.predicted_hits, 0);
        assert_eq!(e.predicted_total, 0);
    }

    #[test]
    fn journal_round_trip_and_invalid_journal_are_atomic() {
        let mut e = engine();
        for word in ["e2e4", "e7e5", "g1f3", "b8c6"] {
            play(&mut e, word);
        }
        e.think(
            &SearchLimits {
                max_depth: 3,
                time_ms: 0,
                variants: 1,
            },
            Report::Quiet,
            true,
        );
        let text = e.save_game();
        let mut restored = engine();
        restored.load_game(&text).unwrap();
        assert_eq!(restored.board.to_fen(), e.board.to_fen());
        assert_eq!(restored.hashes, e.hashes);
        assert_eq!(restored.plan, e.plan);
        assert_eq!(restored.save_game(), text);
        let corrupted = text.replace("moves e2e4", "moves e2e5");
        assert!(restored.load_game(&corrupted).is_err());
        assert_eq!(restored.save_game(), text);
        assert!(restored.undo());
        assert_eq!(restored.history.len(), 3);
    }

    #[test]
    fn seeded_openings_are_repeatable_varied_and_bounded() {
        let mut a = engine();
        let mut b = engine();
        let limits = SearchLimits {
            max_depth: 1,
            time_ms: 0,
            variants: 1,
        };
        let baseline = a.think(&limits, Report::Quiet, false).score;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..24 {
            a.new_game();
            b.new_game();
            let x = a.think(&limits, Report::Quiet, true);
            let y = b.think(&limits, Report::Quiet, true);
            assert_eq!(x.best, y.best);
            assert!(x.score >= baseline - a.config().opening_variety);
            seen.insert(x.best.to_uci());
        }
        assert!(seen.len() >= 2, "opening choices: {seen:?}");
    }
}
