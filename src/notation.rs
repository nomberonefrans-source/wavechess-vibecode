//! Запись ходов: алгебраическая (SAN) и координатная, разбор ввода пользователя.

use crate::board::*;
use crate::geometry::*;
use crate::search::MATE;
use crate::search::MATE_BOUND;

fn file_char(s: usize) -> char {
    (b'a' + (s & 7) as u8) as char
}

fn rank_char(s: usize) -> char {
    (b'1' + (s >> 3) as u8) as char
}

/// Ход в алгебраической записи: Nf3, exd5, O-O, e8=Q+, Qxf7#.
pub fn move_to_san(b: &Board, m: Move) -> String {
    let from = m.from as usize;
    let to = m.to as usize;
    let mut s = String::new();
    if m.flag == FLAG_CASTLE_K {
        s.push_str("O-O");
    } else if m.flag == FLAG_CASTLE_Q {
        s.push_str("O-O-O");
    } else {
        let p = b.sq[from].abs();
        let is_cap = b.sq[to] != 0 || m.flag == FLAG_EP;
        if p == PAWN {
            if is_cap {
                s.push(file_char(from));
                s.push('x');
            }
            s.push_str(&square_name(to));
            if m.promo != 0 {
                s.push('=');
                s.push(piece_letter(m.promo as i8));
            }
        } else {
            s.push(piece_letter(p));
            let legal = b.legal_moves();
            let mut clash = false;
            let mut same_file = false;
            let mut same_rank = false;
            for i in 0..legal.n {
                let o = legal.m[i];
                if o.to == m.to && o.from != m.from && b.sq[o.from as usize].abs() == p {
                    clash = true;
                    if file_char(o.from as usize) == file_char(from) {
                        same_file = true;
                    }
                    if rank_char(o.from as usize) == rank_char(from) {
                        same_rank = true;
                    }
                }
            }
            if clash {
                if !same_file {
                    s.push(file_char(from));
                } else if !same_rank {
                    s.push(rank_char(from));
                } else {
                    s.push(file_char(from));
                    s.push(rank_char(from));
                }
            }
            if is_cap {
                s.push('x');
            }
            s.push_str(&square_name(to));
        }
    }
    let nb = b.make(m);
    if nb.in_check() {
        if nb.legal_moves().n == 0 {
            s.push('#');
        } else {
            s.push('+');
        }
    }
    s
}

fn normalize(t: &str) -> String {
    t.chars()
        .map(|c| if c == '0' { 'O' } else { c })
        .filter(|c| !matches!(*c, '+' | '#' | '!' | '?' | 'x' | '=' | ':' | '-'))
        .collect::<String>()
}

/// Разбор хода из текста: e2e4, e7e8q, Nf3, exd5, O-O, 0-0-0 …
pub fn parse_move(b: &Board, text: &str) -> Option<Move> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let legal = b.legal_moves();
    let tl = t.to_lowercase();
    if tl.len() == 4 || tl.len() == 5 {
        for i in 0..legal.n {
            if legal.m[i].to_uci() == tl {
                return Some(legal.m[i]);
            }
        }
        if tl.len() == 4 {
            let with_q = format!("{}q", tl);
            for i in 0..legal.n {
                if legal.m[i].to_uci() == with_q {
                    return Some(legal.m[i]);
                }
            }
        }
    }
    let norm = normalize(t);
    for i in 0..legal.n {
        let san = move_to_san(b, legal.m[i]);
        if normalize(&san) == norm {
            return Some(legal.m[i]);
        }
    }
    // «nf3» вместо «Nf3»
    if let Some(c0) = norm.chars().next() {
        if matches!(c0, 'n' | 'r' | 'q' | 'k') && norm.len() >= 3 {
            let mut fixed = String::new();
            fixed.push(c0.to_ascii_uppercase());
            fixed.push_str(&norm[1..]);
            for i in 0..legal.n {
                let san = move_to_san(b, legal.m[i]);
                if normalize(&san) == fixed {
                    return Some(legal.m[i]);
                }
            }
        }
    }
    // «e8q» вместо «e8=Q»
    let norm_up = {
        let mut cs: Vec<char> = norm.chars().collect();
        if let Some(last) = cs.last_mut() {
            *last = last.to_ascii_uppercase();
        }
        cs.into_iter().collect::<String>()
    };
    for i in 0..legal.n {
        if legal.m[i].promo != 0 {
            let san = move_to_san(b, legal.m[i]);
            if normalize(&san) == norm_up {
                return Some(legal.m[i]);
            }
        }
    }
    None
}

/// Цепочка ходов в SAN с номерами: «1. e4 e5 2. Nf3 Nc6».
pub fn pv_to_san(b: &Board, pv: &[Move], max: usize) -> String {
    let mut cur = *b;
    let mut out = String::new();
    let mut first = true;
    for (i, &mv) in pv.iter().enumerate() {
        if i >= max {
            break;
        }
        if cur.side == 1 {
            out.push_str(&format!("{}. ", cur.fullmove));
        } else if first {
            out.push_str(&format!("{}... ", cur.fullmove));
        }
        first = false;
        out.push_str(&move_to_san(&cur, mv));
        out.push(' ');
        cur = cur.make(mv);
    }
    out.trim_end().to_string()
}

/// Позиция в конце цепочки.
pub fn apply_pv(b: &Board, pv: &[Move]) -> Board {
    let mut cur = *b;
    for &mv in pv.iter() {
        cur = cur.make(mv);
    }
    cur
}

/// Оценка поиска (с точки зрения ходящего) -> текст с точки зрения белых.
pub fn format_score(score: i32, side_to_move: i8) -> String {
    let w = if side_to_move == 1 { score } else { -score };
    if score > MATE_BOUND || score < -MATE_BOUND {
        let n = (MATE - score.abs() + 1) / 2;
        let winner_white = (score > 0) == (side_to_move == 1);
        if winner_white {
            format!("белые ставят мат в {}", n)
        } else {
            format!("чёрные ставят мат в {}", n)
        }
    } else {
        format!("{:+.2}", w as f32 / 100.0)
    }
}
