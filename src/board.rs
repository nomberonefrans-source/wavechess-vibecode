//! Правила шахмат, выведенные из волновой геометрии.
//!
//! Доска хранится как «поле зарядов»: на клетке лежит знаковое число
//! (+ белая, - чёрная; модуль — тип фигуры). Это не битборд, а обычная решётка чисел.
//! Ходы получаются как распространение волны фигуры по решётке:
//!   * прыжковая волна (конь, король),
//!   * лучевая волна, поглощаемая первой встреченной фигурой (слон, ладья, ферзь),
//!   * пешечная волна (вперёд — только на пустое, по диагонали — только на врага).
//! Шах — это «чужая волна достигла клетки короля». Ход легален, если после него
//! волна противника не достигает собственного короля.

use crate::geometry::*;

pub const FLAG_NONE: u8 = 0;
pub const FLAG_DOUBLE: u8 = 1;
pub const FLAG_EP: u8 = 2;
pub const FLAG_CASTLE_K: u8 = 3;
pub const FLAG_CASTLE_Q: u8 = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Move {
    pub from: u8,
    pub to: u8,
    pub promo: u8,
    pub flag: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_en_passant_does_not_change_repetition_hash() {
        for fen in [
            "4k3/8/8/3p4/8/8/8/4K3 w - d6 0 1",
            "k3r3/8/8/3pP3/8/8/8/4K3 w - d6 0 1",
            "4k3/8/8/r4pPK/8/8/8/8 w - f6 0 1",
        ] {
            let with_ep = Board::from_fen(fen).unwrap();
            let mut without_ep = with_ep;
            without_ep.ep = -1;
            assert_eq!(with_ep.hash, without_ep.compute_hash(), "{fen}");
        }
    }

    #[test]
    fn legal_en_passant_changes_repetition_hash() {
        let b = Board::from_fen("4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 1").unwrap();
        let mut without_ep = b;
        without_ep.ep = -1;
        assert_ne!(b.hash, without_ep.compute_hash());
        let moves = b.legal_moves();
        let ep = moves.m[..moves.n]
            .iter()
            .find(|m| m.flag == FLAG_EP)
            .unwrap();
        let after = b.make(*ep);
        assert_eq!(after.hash, after.compute_hash());
    }

    #[test]
    fn malformed_en_passant_cannot_capture_an_absent_pawn() {
        let b = Board::from_fen("4k3/8/8/4P3/8/8/8/4K3 w - d6 0 1").unwrap();
        let moves = b.legal_moves();
        assert!(moves.m[..moves.n].iter().all(|m| m.flag != FLAG_EP));
    }

    #[test]
    fn same_colour_bishops_are_dead_material() {
        let dead = Board::from_fen("4k3/8/8/8/5b2/8/3B4/4K3 w - - 0 1").unwrap();
        assert!(dead.insufficient());
        let live = Board::from_fen("4k3/8/8/8/4b3/8/3B4/4K3 w - - 0 1").unwrap();
        assert!(!live.insufficient());
        let two_knights = Board::from_fen("4k3/8/8/8/8/8/3NN3/4K3 w - - 0 1").unwrap();
        assert!(!two_knights.insufficient());
    }

    #[test]
    fn move_counters_saturate() {
        let b = Board::from_fen("4k3/8/8/8/8/8/8/R3K3 b - - 65535 65535").unwrap();
        let after = b.make(b.legal_moves().m[0]);
        assert_eq!(after.halfmove, u16::MAX);
        assert_eq!(after.fullmove, u16::MAX);
        assert_eq!(after.hash, after.compute_hash());
    }
}

impl Move {
    pub const NONE: Move = Move {
        from: 0,
        to: 0,
        promo: 0,
        flag: 0,
    };

    pub fn new(from: usize, to: usize, promo: i8, flag: u8) -> Move {
        Move {
            from: from as u8,
            to: to as u8,
            promo: promo as u8,
            flag,
        }
    }

    pub fn is_none(&self) -> bool {
        self.from == self.to
    }

    /// Запись в координатной форме: e2e4, e7e8q.
    pub fn to_uci(&self) -> String {
        let mut s = format!(
            "{}{}",
            square_name(self.from as usize),
            square_name(self.to as usize)
        );
        if self.promo != 0 {
            s.push(piece_letter(self.promo as i8).to_ascii_lowercase());
        }
        s
    }
}

pub struct MoveList {
    pub m: [Move; 256],
    pub n: usize,
}

impl MoveList {
    pub fn new() -> MoveList {
        MoveList {
            m: [Move::NONE; 256],
            n: 0,
        }
    }

    #[inline(always)]
    pub fn push(&mut self, mv: Move) {
        self.m[self.n] = mv;
        self.n += 1;
    }
}

#[derive(Clone, Copy)]
pub struct Board {
    pub sq: [i8; 64],
    pub side: i8,   // 1 — ход белых, -1 — ход чёрных
    pub castle: u8, // биты: 1 — O-O белых, 2 — O-O-O белых, 4 — O-O чёрных, 8 — O-O-O чёрных
    pub ep: i8,     // клетка взятия на проходе или -1
    pub halfmove: u16,
    pub fullmove: u16,
    pub ksq: [u8; 2], // клетки королей: [белый, чёрный]
    pub hash: u64,
}

fn castle_mask(sq: usize) -> u8 {
    match sq {
        0 => 2,
        7 => 1,
        4 => 3,
        56 => 8,
        63 => 4,
        60 => 12,
        _ => 0,
    }
}

fn piece_key(p: i8) -> usize {
    let t = (p.abs() - 1) as usize;
    if p > 0 {
        t
    } else {
        t + 6
    }
}

impl Board {
    pub const START_FEN: &'static str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

    pub fn startpos() -> Board {
        Board::from_fen(Board::START_FEN).unwrap()
    }

    #[inline(always)]
    pub fn side_idx(&self) -> usize {
        if self.side == 1 {
            0
        } else {
            1
        }
    }

    pub fn from_fen(fen: &str) -> Result<Board, String> {
        let parts: Vec<&str> = fen.split_whitespace().collect();
        if parts.len() < 4 {
            return Err("FEN: слишком мало полей".to_string());
        }
        let mut b = Board {
            sq: [0; 64],
            side: 1,
            castle: 0,
            ep: -1,
            halfmove: 0,
            fullmove: 1,
            ksq: [0, 0],
            hash: 0,
        };
        let rows: Vec<&str> = parts[0].split('/').collect();
        if rows.len() != 8 {
            return Err("FEN: нужно 8 рядов".to_string());
        }
        for (i, row) in rows.iter().enumerate() {
            let r = 7 - (i as i8);
            let mut f: i8 = 0;
            for ch in row.chars() {
                if let Some(d) = ch.to_digit(10) {
                    f += d as i8;
                } else {
                    let t = match ch.to_ascii_lowercase() {
                        'p' => PAWN,
                        'n' => KNIGHT,
                        'b' => BISHOP,
                        'r' => ROOK,
                        'q' => QUEEN,
                        'k' => KING,
                        _ => return Err(format!("FEN: неизвестная фигура '{}'", ch)),
                    };
                    if f > 7 {
                        return Err("FEN: ряд слишком длинный".to_string());
                    }
                    let sgn: i8 = if ch.is_ascii_uppercase() { 1 } else { -1 };
                    b.sq[sq_of(f, r)] = t * sgn;
                    f += 1;
                }
            }
            if f != 8 {
                return Err("FEN: ряд должен содержать 8 клеток".to_string());
            }
        }
        b.side = match parts[1] {
            "w" => 1,
            "b" => -1,
            _ => return Err("FEN: ход должен быть w или b".to_string()),
        };
        for ch in parts[2].chars() {
            match ch {
                'K' => b.castle |= 1,
                'Q' => b.castle |= 2,
                'k' => b.castle |= 4,
                'q' => b.castle |= 8,
                _ => {}
            }
        }
        if parts[3] != "-" {
            match parse_square(parts[3]) {
                Some(s) => b.ep = s as i8,
                None => return Err("FEN: плохая клетка взятия на проходе".to_string()),
            }
        }
        if parts.len() > 4 {
            b.halfmove = parts[4].parse::<u16>().unwrap_or(0);
        }
        if parts.len() > 5 {
            b.fullmove = parts[5].parse::<u16>().unwrap_or(1);
        }
        b.finalize()?;
        Ok(b)
    }

    /// Находит королей и считает отпечаток позиции.
    pub fn finalize(&mut self) -> Result<(), String> {
        let mut wk = 0;
        let mut bk = 0;
        for s in 0..64 {
            if self.sq[s] == KING {
                wk += 1;
                self.ksq[0] = s as u8;
            }
            if self.sq[s] == -KING {
                bk += 1;
                self.ksq[1] = s as u8;
            }
        }
        if wk != 1 || bk != 1 {
            return Err("FEN: на доске должно быть по одному королю".to_string());
        }
        self.hash = self.compute_hash();
        Ok(())
    }

    pub fn to_fen(&self) -> String {
        let mut s = String::new();
        for r in (0..8i8).rev() {
            let mut empty = 0;
            for f in 0..8i8 {
                let p = self.sq[sq_of(f, r)];
                if p == 0 {
                    empty += 1;
                } else {
                    if empty > 0 {
                        s.push_str(&empty.to_string());
                        empty = 0;
                    }
                    s.push(piece_char(p));
                }
            }
            if empty > 0 {
                s.push_str(&empty.to_string());
            }
            if r > 0 {
                s.push('/');
            }
        }
        s.push(' ');
        s.push(if self.side == 1 { 'w' } else { 'b' });
        s.push(' ');
        if self.castle == 0 {
            s.push('-');
        } else {
            if self.castle & 1 != 0 {
                s.push('K');
            }
            if self.castle & 2 != 0 {
                s.push('Q');
            }
            if self.castle & 4 != 0 {
                s.push('k');
            }
            if self.castle & 8 != 0 {
                s.push('q');
            }
        }
        s.push(' ');
        if self.ep >= 0 {
            s.push_str(&square_name(self.ep as usize));
        } else {
            s.push('-');
        }
        s.push_str(&format!(" {} {}", self.halfmove, self.fullmove));
        s
    }

    pub fn compute_hash(&self) -> u64 {
        let mut h: u64 = 0;
        for s in 0..64 {
            let p = self.sq[s];
            if p != 0 {
                h ^= KEYS[piece_key(p) * 64 + s];
            }
        }
        if self.side == -1 {
            h ^= KEYS[768];
        }
        h ^= KEYS[769 + (self.castle as usize)];
        if let Some(file) = self.legal_ep_file() {
            h ^= KEYS[785 + file];
        }
        h
    }

    /// Для повторений учитывается только право на действительно легальное взятие.
    /// Недоступная или связанная пешка не меняет множество допустимых ходов.
    fn legal_ep_file(&self) -> Option<usize> {
        if self.ep < 0 || self.ep >= 64 {
            return None;
        }
        let to = self.ep as usize;
        let required_rank = if self.side == 1 { 5 } else { 2 };
        if rank_of(to) != required_rank || self.sq[to] != 0 {
            return None;
        }
        let rank = rank_of(to) - self.side;
        let captured = sq_of(file_of(to), rank);
        if self.sq[captured] != -self.side * PAWN {
            return None;
        }
        for df in [-1i8, 1i8] {
            let file = file_of(to) + df;
            if !inside(file, rank) {
                continue;
            }
            let from = sq_of(file, rank);
            if self.sq[from] != self.side * PAWN {
                continue;
            }
            let mut after = *self;
            after.sq[from] = 0;
            after.sq[captured] = 0;
            after.sq[to] = self.side * PAWN;
            if !after.attacked(self.ksq[self.side_idx()] as usize, -self.side) {
                return Some(file_of(to) as usize);
            }
        }
        None
    }

    /// Обратная волна: достигает ли клетки `s` волна фигур цвета `by`?
    /// Волны обратимы: лучи и прыжки из клетки `s` попадают в источник тем же путём.
    pub fn attacked(&self, s: usize, by: i8) -> bool {
        let f = file_of(s);
        let r = rank_of(s);
        // пешечная волна идёт «вперёд» источника, значит источник стоит «позади» клетки
        let pr = r - by;
        for df in [-1i8, 1i8] {
            let nf = f + df;
            if inside(nf, pr) && self.sq[sq_of(nf, pr)] == by * PAWN {
                return true;
            }
        }
        for &(df, dr) in KNIGHT_JUMPS.iter() {
            let nf = f + df;
            let nr = r + dr;
            if inside(nf, nr) && self.sq[sq_of(nf, nr)] == by * KNIGHT {
                return true;
            }
        }
        for &(df, dr) in KING_STEPS.iter() {
            let nf = f + df;
            let nr = r + dr;
            if inside(nf, nr) && self.sq[sq_of(nf, nr)] == by * KING {
                return true;
            }
        }
        for &(df, dr) in DIAG.iter() {
            let mut nf = f + df;
            let mut nr = r + dr;
            while inside(nf, nr) {
                let q = self.sq[sq_of(nf, nr)];
                if q != 0 {
                    if q == by * BISHOP || q == by * QUEEN {
                        return true;
                    }
                    break;
                }
                nf += df;
                nr += dr;
            }
        }
        for &(df, dr) in ORTHO.iter() {
            let mut nf = f + df;
            let mut nr = r + dr;
            while inside(nf, nr) {
                let q = self.sq[sq_of(nf, nr)];
                if q != 0 {
                    if q == by * ROOK || q == by * QUEEN {
                        return true;
                    }
                    break;
                }
                nf += df;
                nr += dr;
            }
        }
        false
    }

    pub fn in_check(&self) -> bool {
        self.attacked(self.ksq[self.side_idx()] as usize, -self.side)
    }

    /// Недостаточно материала для мата (король против короля, король + лёгкая фигура против короля).
    pub fn insufficient(&self) -> bool {
        let mut minors = 0;
        let mut knights = 0;
        let mut bishop_colour = None;
        let mut same_colour = true;
        for s in 0..64 {
            let t = self.sq[s].abs();
            if t == PAWN || t == ROOK || t == QUEEN {
                return false;
            }
            if t == KNIGHT || t == BISHOP {
                minors += 1;
            }
            if t == KNIGHT {
                knights += 1;
            }
            if t == BISHOP {
                let colour = (file_of(s) + rank_of(s)) & 1;
                if let Some(previous) = bishop_colour {
                    same_colour &= colour == previous;
                } else {
                    bishop_colour = Some(colour);
                }
            }
        }
        minors <= 1 || (knights == 0 && same_colour)
    }

    /// Все псевдолегальные ходы: волны фигур без проверки безопасности своего короля.
    pub fn gen_pseudo(&self, list: &mut MoveList, caps_only: bool) {
        let us = self.side;
        for from in 0..64usize {
            let p = self.sq[from];
            if p == 0 || p.signum() != us {
                continue;
            }
            let t = p.abs();
            if t == PAWN {
                self.gen_pawn(from, list, caps_only);
                continue;
            }
            let f = file_of(from);
            let r = rank_of(from);
            for &(df, dr) in leaper_steps(t) {
                let nf = f + df;
                let nr = r + dr;
                if !inside(nf, nr) {
                    continue;
                }
                let to = sq_of(nf, nr);
                let q = self.sq[to];
                if q == 0 {
                    if !caps_only {
                        list.push(Move::new(from, to, 0, FLAG_NONE));
                    }
                } else if q.signum() != us {
                    list.push(Move::new(from, to, 0, FLAG_NONE));
                }
            }
            for &(df, dr) in slider_rays(t) {
                let mut nf = f + df;
                let mut nr = r + dr;
                while inside(nf, nr) {
                    let to = sq_of(nf, nr);
                    let q = self.sq[to];
                    if q == 0 {
                        if !caps_only {
                            list.push(Move::new(from, to, 0, FLAG_NONE));
                        }
                    } else {
                        if q.signum() != us {
                            list.push(Move::new(from, to, 0, FLAG_NONE));
                        }
                        break;
                    }
                    nf += df;
                    nr += dr;
                }
            }
        }
        if !caps_only {
            self.gen_castles(list);
        }
    }

    fn gen_pawn(&self, from: usize, list: &mut MoveList, caps_only: bool) {
        let us = self.side;
        let f = file_of(from);
        let r = rank_of(from);
        let nr = r + us;
        if nr < 0 || nr > 7 {
            return;
        }
        let last: i8 = if us == 1 { 7 } else { 0 };
        let start: i8 = if us == 1 { 1 } else { 6 };
        // волна вперёд: только на пустую клетку
        let to = sq_of(f, nr);
        if self.sq[to] == 0 {
            if nr == last {
                list.push(Move::new(from, to, QUEEN, FLAG_NONE));
                if !caps_only {
                    list.push(Move::new(from, to, ROOK, FLAG_NONE));
                    list.push(Move::new(from, to, BISHOP, FLAG_NONE));
                    list.push(Move::new(from, to, KNIGHT, FLAG_NONE));
                }
            } else if !caps_only {
                list.push(Move::new(from, to, 0, FLAG_NONE));
                if r == start {
                    let to2 = sq_of(f, r + 2 * us);
                    if self.sq[to2] == 0 {
                        list.push(Move::new(from, to2, 0, FLAG_DOUBLE));
                    }
                }
            }
        }
        // волна по диагонали: только на врага (или на клетку взятия на проходе)
        for df in [-1i8, 1i8] {
            let nf = f + df;
            if !inside(nf, nr) {
                continue;
            }
            let to = sq_of(nf, nr);
            let q = self.sq[to];
            if q != 0 && q.signum() != us {
                if nr == last {
                    list.push(Move::new(from, to, QUEEN, FLAG_NONE));
                    if !caps_only {
                        list.push(Move::new(from, to, ROOK, FLAG_NONE));
                        list.push(Move::new(from, to, BISHOP, FLAG_NONE));
                        list.push(Move::new(from, to, KNIGHT, FLAG_NONE));
                    }
                } else {
                    list.push(Move::new(from, to, 0, FLAG_NONE));
                }
            } else if q == 0
                && self.ep == to as i8
                && r == if us == 1 { 4 } else { 3 }
                && self.sq[sq_of(nf, r)] == -us * PAWN
            {
                list.push(Move::new(from, to, 0, FLAG_EP));
            }
        }
    }

    fn gen_castles(&self, list: &mut MoveList) {
        let us = self.side;
        let them = -us;
        let (rb, kbit, qbit): (usize, u8, u8) = if us == 1 { (0, 1, 2) } else { (56, 4, 8) };
        if self.sq[rb + 4] != KING * us {
            return;
        }
        if self.castle & (kbit | qbit) == 0 {
            return;
        }
        if self.attacked(rb + 4, them) {
            return;
        }
        if self.castle & kbit != 0
            && self.sq[rb + 7] == ROOK * us
            && self.sq[rb + 5] == 0
            && self.sq[rb + 6] == 0
            && !self.attacked(rb + 5, them)
            && !self.attacked(rb + 6, them)
        {
            list.push(Move::new(rb + 4, rb + 6, 0, FLAG_CASTLE_K));
        }
        if self.castle & qbit != 0
            && self.sq[rb] == ROOK * us
            && self.sq[rb + 1] == 0
            && self.sq[rb + 2] == 0
            && self.sq[rb + 3] == 0
            && !self.attacked(rb + 3, them)
            && !self.attacked(rb + 2, them)
        {
            list.push(Move::new(rb + 4, rb + 2, 0, FLAG_CASTLE_Q));
        }
    }

    /// Применяет ход и возвращает новую позицию (доска мала, копирование дёшево).
    /// Отпечаток позиции обновляется только по изменённым клеткам (xor), без пересчёта всей доски.
    pub fn make(&self, m: Move) -> Board {
        let mut b = *self;
        let from = m.from as usize;
        let to = m.to as usize;
        let us = self.side;
        let piece = b.sq[from];
        let captured = b.sq[to];
        let mut h = self.hash;
        h ^= KEYS[piece_key(piece) * 64 + from];
        if captured != 0 {
            h ^= KEYS[piece_key(captured) * 64 + to];
        }
        b.sq[from] = 0;
        b.ep = -1;
        let placed = if m.promo != 0 {
            us * (m.promo as i8)
        } else {
            piece
        };
        b.sq[to] = placed;
        h ^= KEYS[piece_key(placed) * 64 + to];
        match m.flag {
            FLAG_EP => {
                let cs = if us == 1 { to - 8 } else { to + 8 };
                h ^= KEYS[piece_key(b.sq[cs]) * 64 + cs];
                b.sq[cs] = 0;
            }
            FLAG_DOUBLE => {
                b.ep = ((from + to) / 2) as i8;
            }
            FLAG_CASTLE_K => {
                let r = ROOK * us;
                h ^= KEYS[piece_key(r) * 64 + to + 1] ^ KEYS[piece_key(r) * 64 + to - 1];
                b.sq[to + 1] = 0;
                b.sq[to - 1] = r;
            }
            FLAG_CASTLE_Q => {
                let r = ROOK * us;
                h ^= KEYS[piece_key(r) * 64 + to - 2] ^ KEYS[piece_key(r) * 64 + to + 1];
                b.sq[to - 2] = 0;
                b.sq[to + 1] = r;
            }
            _ => {}
        }
        if piece.abs() == KING {
            b.ksq[self.side_idx()] = to as u8;
        }
        b.castle &= !(castle_mask(from) | castle_mask(to));
        if piece.abs() == PAWN || captured != 0 || m.flag == FLAG_EP {
            b.halfmove = 0;
        } else {
            b.halfmove = b.halfmove.saturating_add(1);
        }
        if us == -1 {
            b.fullmove = b.fullmove.saturating_add(1);
        }
        b.side = -us;
        // очередь хода, права рокировки, взятие на проходе
        h ^= KEYS[768];
        h ^= KEYS[769 + (self.castle as usize)] ^ KEYS[769 + (b.castle as usize)];
        if let Some(file) = self.legal_ep_file() {
            h ^= KEYS[785 + file];
        }
        if let Some(file) = b.legal_ep_file() {
            h ^= KEYS[785 + file];
        }
        b.hash = h;
        b
    }

    pub fn legal_moves(&self) -> MoveList {
        let mut pseudo = MoveList::new();
        self.gen_pseudo(&mut pseudo, false);
        let mut out = MoveList::new();
        let ui = self.side_idx();
        for i in 0..pseudo.n {
            let mv = pseudo.m[i];
            let nb = self.make(mv);
            if !nb.attacked(nb.ksq[ui] as usize, -self.side) {
                out.push(mv);
            }
        }
        out
    }

    pub fn perft(&self, depth: u32) -> u64 {
        if depth == 0 {
            return 1;
        }
        let mut list = MoveList::new();
        self.gen_pseudo(&mut list, false);
        let ui = self.side_idx();
        let mut n = 0u64;
        for i in 0..list.n {
            let nb = self.make(list.m[i]);
            if nb.attacked(nb.ksq[ui] as usize, -self.side) {
                continue;
            }
            if depth == 1 {
                n += 1;
            } else {
                n += nb.perft(depth - 1);
            }
        }
        n
    }

    /// Зеркало доски: меняет цвета и переворачивает ряды (для проверки симметрии оценки).
    pub fn mirrored(&self) -> Board {
        let mut b = *self;
        for s in 0..64usize {
            let f = file_of(s);
            let r = rank_of(s);
            b.sq[sq_of(f, 7 - r)] = -self.sq[s];
        }
        b.side = -self.side;
        b.castle = ((self.castle & 3) << 2) | ((self.castle >> 2) & 3);
        if self.ep >= 0 {
            let e = self.ep as usize;
            b.ep = sq_of(file_of(e), 7 - rank_of(e)) as i8;
        }
        let _ = b.finalize();
        b
    }
}
