//! Геометрия доски как чистая математика.
//!
//! Доска — это целочисленная решётка 8x8 с координатами (файл, ряд).
//! Каждая фигура описывается ТОЛЬКО векторами: прыжки (скачки) и лучи (направления распространения).
//! Никаких битбордов и таблиц ходов — только векторная арифметика.

pub const PAWN: i8 = 1;
pub const KNIGHT: i8 = 2;
pub const BISHOP: i8 = 3;
pub const ROOK: i8 = 4;
pub const QUEEN: i8 = 5;
pub const KING: i8 = 6;

/// 8 прыжков коня (векторы).
pub static KNIGHT_JUMPS: [(i8, i8); 8] = [
    (1, 2),
    (2, 1),
    (2, -1),
    (1, -2),
    (-1, -2),
    (-2, -1),
    (-2, 1),
    (-1, 2),
];
/// 8 единичных шагов вокруг клетки (король, а также лучи ферзя).
pub static KING_STEPS: [(i8, i8); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];
/// 4 ортогональных луча (ладья).
pub static ORTHO: [(i8, i8); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
/// 4 диагональных луча (слон).
pub static DIAG: [(i8, i8); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];
static NO_DIRS: [(i8, i8); 0] = [];

/// Направления распространения «длинной» волны фигуры (пусто, если фигура не дальнобойная).
pub fn slider_rays(t: i8) -> &'static [(i8, i8)] {
    match t {
        BISHOP => &DIAG[..],
        ROOK => &ORTHO[..],
        QUEEN => &KING_STEPS[..],
        _ => &NO_DIRS[..],
    }
}

/// Прыжки «короткой» волны фигуры (конь, король).
pub fn leaper_steps(t: i8) -> &'static [(i8, i8)] {
    match t {
        KNIGHT => &KNIGHT_JUMPS[..],
        KING => &KING_STEPS[..],
        _ => &NO_DIRS[..],
    }
}

#[inline(always)]
pub fn file_of(s: usize) -> i8 {
    (s & 7) as i8
}

#[inline(always)]
pub fn rank_of(s: usize) -> i8 {
    (s >> 3) as i8
}

#[inline(always)]
pub fn inside(f: i8, r: i8) -> bool {
    (f as u8) < 8 && (r as u8) < 8
}

#[inline(always)]
pub fn sq_of(f: i8, r: i8) -> usize {
    (r as usize) * 8 + (f as usize)
}

pub fn square_name(s: usize) -> String {
    let file = (b'a' + (s & 7) as u8) as char;
    let rank = (s >> 3) + 1;
    format!("{}{}", file, rank)
}

pub fn parse_square(txt: &str) -> Option<usize> {
    let b = txt.as_bytes();
    if b.len() != 2 {
        return None;
    }
    if b[0] < b'a' || b[0] > b'h' || b[1] < b'1' || b[1] > b'8' {
        return None;
    }
    Some(((b[1] - b'1') as usize) * 8 + (b[0] - b'a') as usize)
}

pub fn piece_letter(t: i8) -> char {
    match t {
        1 => 'P',
        2 => 'N',
        3 => 'B',
        4 => 'R',
        5 => 'Q',
        6 => 'K',
        _ => '?',
    }
}

/// Символ фигуры на доске: большие — белые, маленькие — чёрные, '.' — пусто.
pub fn piece_char(p: i8) -> char {
    if p == 0 {
        '.'
    } else {
        let c = piece_letter(p.abs());
        if p > 0 {
            c
        } else {
            c.to_ascii_lowercase()
        }
    }
}

// ---------------------------------------------------------------------------
// Фазовый «отпечаток» позиции: детерминированные константы из формулы splitmix64.
// (Не случайные числа и не обучение — чистая арифметика над целыми.)
// ---------------------------------------------------------------------------

const fn splitmix64(x0: u64) -> u64 {
    let x = x0.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

const fn build_keys() -> [u64; 800] {
    let mut keys = [0u64; 800];
    let mut i = 0;
    while i < 800 {
        keys[i] = splitmix64(i as u64 + 1);
        i += 1;
    }
    keys
}

/// 0..768 — фигуры (12 видов * 64 клетки), 768 — очередь хода,
/// 769..785 — права рокировки, 785..793 — вертикаль взятия на проходе.
pub static KEYS: [u64; 800] = build_keys();
