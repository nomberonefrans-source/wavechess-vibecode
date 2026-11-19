//! Geometry shared by all moves from a position. A possible check is always
//! confirmed on the child board before it affects legality or evaluation.
use crate::board::{Board, Move, FLAG_CASTLE_K, FLAG_CASTLE_Q, FLAG_EP};
use crate::geometry::*;

pub(crate) struct CheckMap {
    king: usize,
    already_checked: bool,
    ray: [u8; 64],
    opens_line: [bool; 64],
}

impl CheckMap {
    pub fn new(b: &Board, already_checked: bool) -> Self {
        let king = b.ksq[1 - b.side_idx()] as usize;
        let mut map = Self {
            king,
            already_checked,
            ray: [0; 64],
            opens_line: [false; 64],
        };
        for &(df, dr) in &KING_STEPS {
            let kind = if df != 0 && dr != 0 { BISHOP } else { ROOK };
            let (mut f, mut r) = (file_of(king) + df, rank_of(king) + dr);
            let mut blocker = None;
            while inside(f, r) {
                let s = sq_of(f, r);
                let p = b.sq[s];
                if let Some(first) = blocker {
                    if p != 0 {
                        map.opens_line[first] = p == b.side * kind || p == b.side * QUEEN;
                        break;
                    }
                } else {
                    map.ray[s] = kind as u8;
                    if p != 0 {
                        if p.signum() != b.side {
                            break;
                        }
                        blocker = Some(s);
                    }
                }
                f += df;
                r += dr;
            }
        }
        map
    }

    #[inline]
    pub fn possible(&self, b: &Board, mv: Move) -> bool {
        let from = mv.from as usize;
        if self.already_checked
            || self.opens_line[from]
            || mv.promo != 0
            || matches!(mv.flag, FLAG_EP | FLAG_CASTLE_K | FLAG_CASTLE_Q)
        {
            return true;
        }
        let to = mv.to as usize;
        let df = (file_of(to) - file_of(self.king)).abs();
        let dr = rank_of(self.king) - rank_of(to);
        match b.sq[from].abs() {
            PAWN => df == 1 && dr == b.side,
            KNIGHT => df * dr.abs() == 2,
            BISHOP | ROOK => self.ray[to] == b.sq[from].unsigned_abs(),
            QUEEN => self.ray[to] != 0,
            _ => df <= 1 && dr.abs() <= 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_actual_check_survives_the_map() {
        let mut b = Board::startpos();
        let mut seed = 987654321u64;
        for _ in 0..1200 {
            let ms = b.legal_moves();
            if ms.n == 0 || b.halfmove >= 90 {
                b = Board::startpos();
                continue;
            }
            let king = b.ksq[1 - b.side_idx()] as usize;
            let map = CheckMap::new(&b, b.attacked(king, b.side));
            for &m in &ms.m[..ms.n] {
                assert!(
                    !b.make(m).in_check() || map.possible(&b, m),
                    "{} {}",
                    b.to_fen(),
                    m.to_uci()
                );
            }
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            b = b.make(ms.m[(seed >> 32) as usize % ms.n]);
        }
    }
}
