//! Волновая система движка.
//!
//! Каждая фигура — источник волны с «зарядом» (масса) и «амплитудой» излучения.
//! Волна распространяется по тем же векторам, что и ходы фигуры:
//!   * лучевая волна затухает с расстоянием (коэффициент DAMP),
//!   * первая фигура на пути поглощает прямую волну, но сквозь неё проходит слабая
//!     «рентгеновская» компонента: через свою дальнобойную фигуру (батарея) — XRAY_OWN,
//!     через чужую фигуру (связка, сквозной удар) — XRAY_ENEMY.
//! Суперпозиция всех волн даёт поле — полное знание о доске:
//!   charge — поле масс,
//!   amp[цвет][клетка] — давление волн каждого цвета,
//!   hits — число прямых ударов,
//!   mobility — охват пространства,
//!   геометрия источников — последовательный перенос массы на занятой клетке;
//!     каждый перенос открывает лучи и заново проверяет допустимость следующей волны,
//!   pin — фигура, которую чужая волна «прошила» насквозь до её короля (связка).
//! Позиция оценивается как энергия поля: кто «громче» звучит в важных зонах
//! (центр, окрестность королей), у того перевес. По знаку баланса видно, кто выигрывает.

use crate::board::{Board, Move, FLAG_EP};
use crate::geometry::*;
use crate::params::Ps;
use std::sync::OnceLock;

/// Масса (заряд) фигуры в сотых долях пешки.
pub const MASS: [f32; 6] = [100.0, 320.0, 330.0, 500.0, 900.0, 0.0];
/// Амплитуда излучения: чем дешевле фигура, тем «сильнее» её волна в размене.
const AMP: [f32; 6] = [1.0, 0.55, 0.55, 0.35, 0.20, 0.12];
/// Затухание луча с расстоянием.
const DAMP: [f32; 8] = [1.0, 0.93, 0.8649, 0.8044, 0.7481, 0.6957, 0.6470, 0.6017];

pub struct WaveField {
    pub charge: [f32; 64],
    pub amp: [[f32; 64]; 2],
    pub hits: [[u8; 64]; 2],
    pub mobility: [f32; 2],
    /// Геометрия источников. Снимок заменяет обрезанные стопки: батареи и связки
    /// пересчитываются только для клетки, на которой действительно идёт перенос.
    board: Board,
    /// Связка: 0 — нет; 1 — по вертикали, 2 — по горизонтали, 3 — по диагонали «/», 4 — по диагонали «\».
    pub pin: [u8; 64],
    /// Удары без учёта связок (связанная фигура всё равно «бьёт» поле рядом с королём).
    pub raw_hits: [[u8; 64]; 2],
}

/// Король как бьющая фигура: брать защищённую фигуру он не может, поэтому масса «бесконечная».
pub const KING_ATTACK_MASS: f32 = 20000.0;

struct Tables {
    centre: [f32; 64],
    zone: Vec<[f32; 64]>,
    /// Расстояние от центра доски (0.7 в центре, 4.95 в углу).
    cdist: [f32; 64],
}

static TABLES: OnceLock<Tables> = OnceLock::new();

fn tables() -> &'static Tables {
    TABLES.get_or_init(|| {
        let mut centre = [0f32; 64];
        let mut cdist = [0f32; 64];
        let mut zone = vec![[0f32; 64]; 64];
        for s in 0..64usize {
            let dx = (s & 7) as f32 - 3.5;
            let dy = (s >> 3) as f32 - 3.5;
            // гауссов «колокол» с центром в середине доски
            centre[s] = (-(dx * dx + dy * dy) / 12.0).exp();
            cdist[s] = (dx * dx + dy * dy).sqrt();
        }
        for k in 0..64usize {
            for s in 0..64usize {
                let dx = (k & 7) as f32 - (s & 7) as f32;
                let dy = (k >> 3) as f32 - (s >> 3) as f32;
                // гауссова окрестность вокруг клетки короля
                zone[k][s] = (-(dx * dx + dy * dy) / 4.0).exp();
            }
        }
        Tables {
            centre,
            zone,
            cdist,
        }
    })
}

#[inline(always)]
fn sat(x: f32) -> f32 {
    x / (x + 0.8)
}

#[inline(always)]
fn line_id(df: i8, dr: i8) -> u8 {
    if df == 0 {
        1
    } else if dr == 0 {
        2
    } else if df == dr {
        3
    } else {
        4
    }
}

/// Находит связанные фигуры: луч от короля проходит через свою фигуру и упирается
/// в чужую дальнобойную волну подходящего типа.
fn find_pins(b: &Board, pin: &mut [u8; 64]) {
    for c in 0..2usize {
        let own: i8 = if c == 0 { 1 } else { -1 };
        let ks = b.ksq[c] as usize;
        let kf = file_of(ks);
        let kr = rank_of(ks);
        for &(df, dr) in KING_STEPS.iter() {
            let diag = df != 0 && dr != 0;
            let mut nf = kf + df;
            let mut nr = kr + dr;
            let mut first: i32 = -1;
            while inside(nf, nr) {
                let s = sq_of(nf, nr);
                let q = b.sq[s];
                if q != 0 {
                    if first < 0 {
                        if q.signum() == own {
                            first = s as i32;
                        } else {
                            break;
                        }
                    } else {
                        if q.signum() == -own {
                            let t = q.abs();
                            if t == QUEEN || (diag && t == BISHOP) || (!diag && t == ROOK) {
                                pin[first as usize] = line_id(df, dr);
                            }
                        }
                        break;
                    }
                }
                nf += df;
                nr += dr;
            }
        }
    }
}

impl WaveField {
    pub fn compute(b: &Board) -> WaveField {
        Self::compute_with(b, &Ps::load())
    }

    pub fn compute_with(b: &Board, ps: &Ps) -> WaveField {
        let mut w = WaveField {
            charge: [0.0; 64],
            amp: [[0.0; 64]; 2],
            hits: [[0; 64]; 2],
            mobility: [0.0; 2],
            board: *b,
            pin: [0; 64],
            raw_hits: [[0; 64]; 2],
        };
        find_pins(b, &mut w.pin);
        for s in 0..64usize {
            let p = b.sq[s];
            if p == 0 {
                continue;
            }
            let c: usize = if p > 0 { 0 } else { 1 };
            let sg = p.signum();
            let t = p.abs();
            let ti = (t - 1) as usize;
            w.charge[s] = sg as f32 * MASS[ti];
            let a = AMP[ti];
            let f = file_of(s);
            let r = rank_of(s);
            let pn = w.pin[s];
            if t == PAWN {
                let nr = r + sg;
                for df in [-1i8, 1i8] {
                    let nf = f + df;
                    if inside(nf, nr) {
                        let to = sq_of(nf, nr);
                        w.amp[c][to] += a;
                        w.raw_hits[c][to] += 1;
                        if pn == 0 || line_id(df, sg) == pn {
                            w.hits[c][to] += 1;
                        }
                    }
                }
            } else if t == KNIGHT || t == KING {
                for &(df, dr) in leaper_steps(t) {
                    let nf = f + df;
                    let nr = r + dr;
                    if !inside(nf, nr) {
                        continue;
                    }
                    let to = sq_of(nf, nr);
                    w.amp[c][to] += a;
                    w.raw_hits[c][to] += 1;
                    if t == KNIGHT && pn != 0 {
                        continue; // связанный конь не излучает
                    }
                    w.hits[c][to] += 1;
                    if t == KNIGHT && b.sq[to].signum() != sg {
                        w.mobility[c] += 4.0;
                    }
                }
            } else {
                let mw: f32 = match t {
                    BISHOP => 4.0,
                    ROOK => 2.5,
                    _ => 1.0,
                };
                for &(df, dr) in slider_rays(t) {
                    let allowed = pn == 0 || line_id(df, dr) == pn;
                    let mut nf = f + df;
                    let mut nr = r + dr;
                    let mut atten = 1.0f32;
                    let mut direct = true;
                    let mut d = 0usize;
                    while inside(nf, nr) {
                        let to = sq_of(nf, nr);
                        let q = b.sq[to];
                        let dd = if d > 7 { 7 } else { d };
                        w.amp[c][to] += a * atten * DAMP[dd];
                        if direct {
                            w.raw_hits[c][to] += 1;
                        }
                        if allowed {
                            if direct {
                                w.hits[c][to] += 1;
                                if q.signum() != sg {
                                    w.mobility[c] += mw;
                                }
                            }
                        }
                        if q != 0 {
                            direct = false;
                            if q.signum() == sg {
                                let qt = q.abs();
                                if qt == BISHOP || qt == ROOK || qt == QUEEN {
                                    atten *= ps.xray_own;
                                } else {
                                    break;
                                }
                            } else {
                                atten *= ps.xray_enemy;
                            }
                        }
                        nf += df;
                        nr += dr;
                        d += 1;
                    }
                }
            }
        }
        w
    }
}

/// Один допустимый перенос источника на клетку столкновения. Масса короля
/// нужна лишь для порядка источников; короля никогда нельзя разменивать.
#[derive(Clone, Copy)]
struct Transfer {
    board: Board,
    mass: f32,
    promotion: f32,
}

fn transfer(b: &Board, from: usize, to: usize, side: i8) -> Transfer {
    let mut next = *b;
    let p = next.sq[from];
    let promoted = p.abs() == PAWN && (rank_of(to) == 0 || rank_of(to) == 7);
    next.sq[from] = 0;
    next.sq[to] = if promoted { side * QUEEN } else { p };
    if p.abs() == KING {
        next.ksq[if side > 0 { 0 } else { 1 }] = to as u8;
    }
    next.side = -side;
    next.ep = -1;
    Transfer {
        board: next,
        mass: if p.abs() == KING {
            KING_ATTACK_MASS
        } else {
            MASS[(next.sq[to].abs() - 1) as usize]
        },
        promotion: if promoted { MASS[4] - MASS[0] } else { 0.0 },
    }
}

/// Наиболее лёгкая допустимая волна, непосредственно достигающая клетки.
/// Заново читаем восемь лучей после каждого переноса: задняя ладья не может
/// вступить раньше передней, а открывшаяся связка исключает ложного защитника.
fn incident(b: &Board, to: usize, side: i8) -> Option<Transfer> {
    let f = file_of(to);
    let r = rank_of(to);
    let mut sources = [0u8; 32];
    let mut count = 0usize;
    let mut add = |nf: i8, nr: i8, kind: i8| {
        if inside(nf, nr) {
            let from = sq_of(nf, nr);
            if b.sq[from] == side * kind {
                sources[count] = from as u8;
                count += 1;
            }
        }
    };
    for df in [-1, 1] {
        add(f + df, r - side, PAWN);
    }
    for &(df, dr) in KNIGHT_JUMPS.iter() {
        add(f + df, r + dr, KNIGHT);
    }
    for &(df, dr) in KING_STEPS.iter() {
        add(f + df, r + dr, KING);
    }
    for &(df, dr) in KING_STEPS.iter() {
        let mut nf = f + df;
        let mut nr = r + dr;
        while inside(nf, nr) {
            let from = sq_of(nf, nr);
            let p = b.sq[from];
            if p != 0 {
                let diagonal = df != 0 && dr != 0;
                if p.signum() == side
                    && (p.abs() == QUEEN
                        || (diagonal && p.abs() == BISHOP)
                        || (!diagonal && p.abs() == ROOK))
                {
                    sources[count] = from as u8;
                    count += 1;
                }
                break;
            }
            nf += df;
            nr += dr;
        }
    }
    let own = if side > 0 { 0 } else { 1 };
    let order = |from: u8| {
        let p = b.sq[from as usize].abs();
        let mass = if p == KING {
            KING_ATTACK_MASS
        } else {
            MASS[(p - 1) as usize]
        };
        // Одинаковый порядок при зеркальном отражении с заменой цветов.
        (mass as u16, if side > 0 { from } else { from ^ 56 })
    };
    for i in 0..count {
        let mut lightest = i;
        for j in (i + 1)..count {
            if order(sources[j]) < order(sources[lightest]) {
                lightest = j;
            }
        }
        sources.swap(i, lightest);
        let t = transfer(b, sources[i] as usize, to, side);
        if !t.board.attacked(t.board.ksq[own] as usize, -side) {
            return Some(t);
        }
    }
    None
}

/// Последовательный локальный поток массы: сторона принимает захват, когда
/// его ближайшая компенсация неотрицательна. Компенсация — масса цели минус
/// масса источника, если есть ответ, плюс масса отвечающего при наличии своего
/// следующего источника. Это локальное приближение: не оптимизация дерева,
/// не обратная свёртка и не гарантия исхода сложной комбинации.
fn flow(
    b: Board,
    to: usize,
    mut target: f32,
    mut side: i8,
    initiator: i8,
    mut balance: f32,
) -> f32 {
    let mut next_wave = incident(&b, to, side);
    // Каждый перенос убирает одну фигуру; даже некорректный плотный FEN ограничен 64 клетками.
    for _ in 0..64 {
        let Some(t) = next_wave else { break };
        let reward = target + t.promotion;
        let response = incident(&t.board, to, -side);
        if let Some(answer) = response {
            if t.mass > reward {
                let refund = if incident(&answer.board, to, side).is_some() {
                    answer.mass
                } else {
                    0.0
                };
                if reward - t.mass + refund < 0.0 {
                    break;
                }
            }
        }
        balance += if side == initiator { reward } else { -reward };
        target = t.mass;
        next_wave = response;
        side = -side;
    }
    balance.max(0.0)
}

/// Ожидаемый положительный поток массы от захвата цели. Проверяются реальные
/// источники, безопасность короля, открывающиеся лучи и превращения пешек.
/// Отказ от невыгодного переноса — локальное правило, без обратной минимаксной свёртки.
pub fn exchange(w: &WaveField, att: usize, def: usize, s: usize, m: f32) -> f32 {
    debug_assert_eq!(att + def, 1);
    if w.hits[att][s] == 0 {
        return 0.0;
    }
    let side = if att == 0 { 1 } else { -1 };
    flow(w.board, s, m, side, side, 0.0)
}

/// Поток для конкретного допустимого взятия. В отличие от угрозы по клетке,
/// учитывает снятие пешки при взятии на проходе и фактическое превращение.
/// Вызывающий передаёт только ход из Board::legal_moves().
pub fn exchange_capture(b: &Board, mv: Move) -> f32 {
    let victim = if mv.flag == FLAG_EP {
        (mv.to as i16 - 8 * b.side as i16) as usize
    } else {
        mv.to as usize
    };
    let p = b.sq[victim].abs();
    if p == 0 || p == KING {
        return 0.0;
    }
    let next = b.make(mv);
    let placed = next.sq[mv.to as usize].abs();
    let target = if placed == KING {
        KING_ATTACK_MASS
    } else {
        MASS[(placed - 1) as usize]
    };
    let promotion = if mv.promo != 0 {
        MASS[(mv.promo - 1) as usize] - MASS[0]
    } else {
        0.0
    };
    flow(
        next,
        mv.to as usize,
        target,
        -b.side,
        b.side,
        MASS[(p - 1) as usize] + promotion,
    )
}

/// Разложение энергии поля по составляющим (в сотых долях пешки, с точки зрения белых).
#[derive(Clone, Copy, Default, Debug)]
pub struct Energy {
    pub mass: f32,
    pub control: f32,
    pub king: f32,
    pub placement: f32,
    pub structure: f32,
    pub mobility: f32,
    pub total: f32,
}

#[inline(always)]
fn cheb(a: usize, b: usize) -> i32 {
    let df = (file_of(a) - file_of(b)).abs() as i32;
    let dr = (rank_of(a) - rank_of(b)).abs() as i32;
    if df > dr {
        df
    } else {
        dr
    }
}

/// Область короля: сколько клеток он может достичь по полю, не входя под удар волн соперника
/// (заливка от клетки короля). Чем она меньше, тем ближе мат.
fn king_region(b: &Board, w: &WaveField, k: usize, loser: usize) -> i32 {
    let winner = 1 - loser;
    let own: i8 = if loser == 0 { 1 } else { -1 };
    let mut seen = [false; 64];
    let mut stack = [0u8; 64];
    let mut sp = 1usize;
    stack[0] = k as u8;
    seen[k] = true;
    let mut n = 0i32;
    while sp > 0 {
        sp -= 1;
        let s = stack[sp] as usize;
        n += 1;
        let f = file_of(s);
        let r = rank_of(s);
        for &(df, dr) in KING_STEPS.iter() {
            let nf = f + df;
            let nr = r + dr;
            if !inside(nf, nr) {
                continue;
            }
            let to = sq_of(nf, nr);
            if seen[to] || b.sq[to].signum() == own || w.raw_hits[winner][to] > 0 {
                continue;
            }
            seen[to] = true;
            stack[sp] = to as u8;
            sp += 1;
        }
    }
    n
}

pub fn analyze(b: &Board) -> (WaveField, Energy) {
    analyze_with(b, &Ps::load())
}

pub fn analyze_with(b: &Board, ps: &Ps) -> (WaveField, Energy) {
    let w = WaveField::compute_with(b, ps);
    let t = tables();
    let mut e = Energy::default();

    let mut npm = 0.0f32;
    let mut mat = [0.0f32; 2];
    for s in 0..64usize {
        let p = b.sq[s].abs();
        if p >= PAWN && p <= QUEEN {
            let m = MASS[(p - 1) as usize];
            mat[if b.sq[s] > 0 { 0 } else { 1 }] += m;
            if p >= KNIGHT {
                npm += m;
            }
        }
    }
    let phase = (npm / 6400.0).min(1.0);

    let wk = b.ksq[0] as usize;
    let bk = b.ksq[1] as usize;

    // 1) масса + контроль пространства + давление вокруг королей
    for s in 0..64usize {
        e.mass += w.charge[s];
        let diff = sat(w.amp[0][s]) - sat(w.amp[1][s]);
        e.control += ps.k_control * (0.4 + t.centre[s]) * diff;
        // Свои волны около собственного короля не создают атакующую энергию:
        // они лишь ослабляют входящий поток. Это не позволяет безопасному на
        // вид центру притягивать короля за счёт собственных активных фигур.
        let white = sat(w.amp[0][s]);
        let black = sat(w.amp[1][s]);
        e.king += ps.k_king
            * (white / (1.0 + black) * t.zone[bk][s] - black / (1.0 + white) * t.zone[wk][s]);
    }

    // 1б) «окно побега» короля: волна противника у короля и свободные клетки вокруг него
    let mut freec = [8i32; 2];
    for c in 0..2usize {
        let ks = b.ksq[c] as usize;
        let en = 1 - c;
        let kf = file_of(ks);
        let kr = rank_of(ks);
        let own: i8 = if c == 0 { 1 } else { -1 };
        let mut expo = sat(w.amp[en][ks]);
        let mut free = 0i32;
        for &(df, dr) in KING_STEPS.iter() {
            let nf = kf + df;
            let nr = kr + dr;
            if !inside(nf, nr) {
                continue;
            }
            let to = sq_of(nf, nr);
            expo += sat(w.amp[en][to]);
            if b.sq[to].signum() != own && w.raw_hits[en][to] == 0 {
                free += 1;
            }
        }
        freec[c] = free;
        let boxed = (4 - free.min(4)) as f32 / 4.0; // 0 — король свободен, 1 — зажат
        let pen = ps.k_flight * expo * boxed * (0.25 + 0.75 * phase);
        // штраф стороне c = плюс противнику
        e.king += if c == 0 { -pen } else { pen };
    }

    // 2) Потенциал источников: один закон для всех фигур. Поддержка другими
    // источниками насыщается; тяжёлый источник требует больше опоры поля.
    // Нет бонусов за тип, пару фигур, открытую вертикаль или номер горизонтали.
    for s in 0..64 {
        let p = b.sq[s];
        if p == 0 {
            continue;
        }
        let c = if p > 0 { 0 } else { 1 };
        let sign = p.signum() as f32;
        if p.abs() == KING {
            e.placement +=
                sign * t.centre[s] * (ps.king_activity * (1.0 - phase) - ps.king_shelter * phase);
            continue;
        }
        let mass = MASS[(p.abs() - 1) as usize];
        let support = sat(w.amp[c][s]);
        e.placement += sign
            * (ps.relief * (100.0 / mass).sqrt() * t.centre[s]
                + ps.cohesion * (1.0 + mass / 100.0).ln() * support);
        if w.pin[s] != 0 {
            e.placement -= sign * ps.k_pin;
        }
        if p.abs() != PAWN {
            continue;
        }

        // 3) Направленный перенос пешечного заряда к границе превращения.
        // Преграда поглощает поток, давление соперника уменьшает пропускание.
        // Задвоенность, изоляция и «проходная» не распознаются отдельными правилами.
        let step = p.signum();
        let distance = if p > 0 { 7 - rank_of(s) } else { rank_of(s) };
        if distance <= 0 {
            continue;
        }
        let f = file_of(s);
        let mut r = rank_of(s) + step;
        let mut transmission = 1.0f32;
        while inside(f, r) {
            let to = sq_of(f, r);
            if b.sq[to] != 0 {
                transmission = 0.0;
                break;
            }
            let own = w.amp[c][to];
            let enemy = w.amp[1 - c][to];
            transmission *= (0.8 + own) / (0.8 + own + enemy);
            r += step;
        }
        let boundary = sq_of(f, if p > 0 { 7 } else { 0 });
        let intercept = (distance as i32 - cheb(b.ksq[1 - c] as usize, boundary)).max(0) as f32;
        let escort = 1.0 / (1.0 + cheb(b.ksq[c] as usize, s) as f32);
        let arrival = transmission / (1.0 + intercept * (1.0 - escort));
        e.structure +=
            sign * ps.transport * (MASS[4] - MASS[0]) * 0.55f32.powi(distance as i32) * arrival;
    }

    // 4) эндшпиль: сильнейший гонит короля слабейшего к краю и подтягивает своего
    let lead = mat[0] - mat[1];
    if lead.abs() >= 250.0 {
        let sign = if lead > 0.0 { 1.0f32 } else { -1.0f32 };
        let (wi, li) = if lead > 0.0 {
            (0usize, 1usize)
        } else {
            (1usize, 0usize)
        };
        let loser_mat = mat[li];
        let lone = (1.0 - loser_mat / 700.0).max(0.0);
        let big = ((lead.abs() - 250.0) / 300.0).min(1.0);
        // при подавляющем перевесе тяга к мату перекрывает остальной шум оценки
        let crush = 1.0 + ((lead.abs() - 600.0) / 600.0).clamp(0.0, 1.0) * ps.crush;
        let scale = lone * big * crush;
        let lk = b.ksq[li] as usize;
        let wkk = b.ksq[wi] as usize;
        let kd = cheb(lk, wkk) as f32;
        e.placement += sign
            * scale
            * (ps.k_edge * t.cdist[lk]
                + ps.k_kprox * (7.0 - kd)
                + ps.k_box * (8 - freec[li]) as f32
                + ps.k_reg * (30 - king_region(b, &w, lk, li).min(30)) as f32);
    }

    e.mobility = ps.k_mob * (w.mobility[0] - w.mobility[1]);
    e.total = e.mass + e.control + e.king + e.placement + e.structure + e.mobility;
    (w, e)
}

/// Оценка в сотых долях пешки с точки зрения белых.
pub fn evaluate(b: &Board) -> i32 {
    let (_, e) = analyze(b);
    e.total.round() as i32
}

/// Доля «волны» белых (0..1): логистическая свёртка баланса.
pub fn white_share(cp: i32) -> f32 {
    1.0 / (1.0 + (-(cp as f32) / 260.0).exp())
}

/// Словесный вердикт по волновому балансу.
pub fn verdict(cp: i32) -> String {
    let a = cp.abs();
    let who = if cp > 0 { "белых" } else { "чёрных" };
    if a < 40 {
        "равенство: волны уравновешены".to_string()
    } else if a < 120 {
        format!("небольшой перевес волны у {}", who)
    } else if a < 300 {
        format!("позиция {} лучше: волна на их стороне", who)
    } else if a < 700 {
        format!(
            "{} в выигрышной позиции",
            if cp > 0 { "белые" } else { "чёрные" }
        )
    } else {
        format!(
            "{} в решающем преимуществе",
            if cp > 0 { "белые" } else { "чёрные" }
        )
    }
}

impl Energy {
    pub fn describe(&self) -> String {
        format!(
            "  масса (материал)          {:+7.2}\n  контроль пространства     {:+7.2}\n  давление у королей        {:+7.2}\n  фигуры в рельефе доски    {:+7.2}\n  поток к превращению       {:+7.2}\n  охват (подвижность волн)  {:+7.2}\n  ---------------------------------\n  суммарная энергия поля    {:+7.2}",
            self.mass / 100.0,
            self.control / 100.0,
            self.king / 100.0,
            self.placement / 100.0,
            self.structure / 100.0,
            self.mobility / 100.0,
            self.total / 100.0
        )
    }
}

/// Карта поля: на каждой клетке фигура и чистое давление волн (белые минус чёрные, x10).
pub fn wave_map(b: &Board) -> String {
    let w = WaveField::compute(b);
    let mut s = String::new();
    s.push_str("      a     b     c     d     e     f     g     h\n");
    for r in (0..8usize).rev() {
        s.push_str(&format!(" {} ", r + 1));
        for f in 0..8usize {
            let sq = r * 8 + f;
            let net = ((w.amp[0][sq] - w.amp[1][sq]) * 10.0).round() as i32;
            let net = net.max(-99).min(99);
            s.push_str(&format!(" {}{:+03} ", piece_char(b.sq[sq]), net));
        }
        s.push('\n');
    }
    s.push_str("  (+ давит белая волна, - давит чёрная; число = разность амплитуд x10)\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn threat(fen: &str, side: usize, square: &str) -> f32 {
        let b = Board::from_fen(fen).unwrap();
        let s = parse_square(square).unwrap();
        let mass = MASS[(b.sq[s].abs() - 1) as usize];
        let w = WaveField::compute(&b);
        let result = exchange(&w, side, 1 - side, s, mass);
        let mirrored = b.mirrored();
        let reflected = WaveField::compute(&mirrored);
        assert_eq!(result, exchange(&reflected, 1 - side, side, s ^ 56, mass));
        result
    }

    fn capture(fen: &str, uci: &str) -> f32 {
        let b = Board::from_fen(fen).unwrap();
        let moves = b.legal_moves();
        let mv = moves.m[..moves.n]
            .iter()
            .find(|m| m.to_uci() == uci)
            .unwrap();
        let result = exchange_capture(&b, *mv);
        let mirrored = b.mirrored();
        let reflected = Move {
            from: mv.from ^ 56,
            to: mv.to ^ 56,
            ..*mv
        };
        assert_eq!(result, exchange_capture(&mirrored, reflected));
        result
    }

    #[test]
    fn undefended_piece_has_full_mass_flux() {
        assert_eq!(threat("k7/8/8/8/4q3/8/8/4R1K1 w - - 0 1", 0, "e4"), 900.0);
    }

    #[test]
    fn poisoned_pawn_does_not_attract_queen() {
        let fen = "k7/8/4p3/3p4/8/8/8/3Q2K1 w - - 0 1";
        assert_eq!(threat(fen, 0, "d5"), 0.0);
        assert_eq!(capture(fen, "d1d5"), 0.0);
    }

    #[test]
    fn king_cannot_absorb_a_protected_wave() {
        assert_eq!(threat("8/8/8/3k4/4Q3/8/8/4R1K1 b - - 0 1", 1, "e4"), 0.0);
    }

    #[test]
    fn pinned_defender_does_not_protect_pawn() {
        assert_eq!(
            threat("3k4/8/3p4/4p3/8/5N2/8/3R2K1 w - - 0 1", 0, "e5"),
            100.0
        );
    }

    #[test]
    fn rear_rook_waits_for_front_queen() {
        assert_eq!(threat("7k/8/8/1p6/r7/8/Q7/R6K w - - 0 1", 0, "a4"), 0.0);
    }

    #[test]
    fn en_passant_removes_the_actual_victim() {
        assert_eq!(capture("7k/8/8/3pP3/8/8/8/6K1 w - d6 0 1", "e5d6"), 100.0);
    }

    #[test]
    fn promotion_flux_includes_created_mass() {
        assert_eq!(capture("4k2r/6P1/8/8/8/8/8/6K1 w - - 0 1", "g7h8q"), 1300.0);
        assert_eq!(capture("4k2r/6P1/8/8/8/8/8/6K1 w - - 0 1", "g7h8n"), 720.0);
    }
}
