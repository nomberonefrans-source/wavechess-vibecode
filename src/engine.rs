//! Движок: позиция + память партии + план (цепочка ходов, построенная наперёд).

use crate::board::*;
use crate::notation;
use crate::search::*;
use crate::settings::EngineConfig;
use crate::wave;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[derive(Clone)]
pub struct MoveRecord {
    pub mv: Move,
    pub san: String,
    pub by: i8,             // кто ходил: 1 белые, -1 чёрные
    pub balance_after: i32, // волновой баланс после хода (в сотых пешки, с точки зрения белых)
    pub predicted: bool,    // был ли этот ход предсказан планом
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
            searcher: Searcher::new(&EngineConfig::default(), stop),
            predicted_hits: 0,
            predicted_total: 0,
        }
    }

    pub fn new_game(&mut self) {
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
    }

    /// Сыграть ход. `by_engine` — ход сделал сам движок (иначе это ответ соперника).
    pub fn play(&mut self, mv: Move, by_engine: bool) {
        let san = notation::move_to_san(&self.board, mv);
        let mut predicted = false;
        if by_engine {
            if !self.plan.is_empty() && self.plan[0] == mv {
                self.plan.remove(0);
            } else {
                self.plan.clear();
            }
        } else if !self.plan.is_empty() {
            self.predicted_total += 1;
            if self.plan[0] == mv {
                predicted = true;
                self.predicted_hits += 1;
                self.plan.remove(0);
            } else {
                self.plan.clear();
            }
        }
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
        });
    }

    pub fn undo(&mut self) -> bool {
        match self.boards.pop() {
            Some(b) => {
                self.board = b;
                self.hashes.pop();
                self.history.pop();
                self.plan.clear();
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
        let res = self
            .searcher
            .think(&self.board, &self.hashes, limits, hint, report);
        if store_plan {
            if res.lines.is_empty() {
                self.plan.clear();
            } else {
                self.plan = res.lines[0].pv.clone();
            }
        }
        res
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
        s
    }
}
