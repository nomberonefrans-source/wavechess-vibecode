//! A forward-wave prior. All coordinates are relative to the mover.
//! One linear score per move, sampled from the shared pressure field.
use crate::board::{Board, Move, MoveList, FLAG_CASTLE_K, FLAG_CASTLE_Q, FLAG_EP};
use crate::geometry::*;
use crate::params::Ps;
use crate::wave::{WaveField, MASS};

include!("policy_weights.rs");

fn distance(a: usize, b: usize) -> f32 {
    (file_of(a) - file_of(b))
        .abs()
        .max((rank_of(a) - rank_of(b)).abs()) as f32
}

fn centre(s: usize) -> f32 {
    7.0 - ((2 * file_of(s) - 7).abs() + (2 * rank_of(s) - 7).abs()) as f32 * 0.5
}

fn value(piece: i8) -> f32 {
    match piece.abs() {
        0 => 0.0,
        KING => 10.0,
        p => MASS[(p - 1) as usize] / 100.0,
    }
}

/// Conditional log-odds from geometry and pressure on the two affected squares.
/// The same field serves every legal move and every term is scalar.
fn preference(b: &Board, w: &WaveField, mv: Move, check: bool) -> f32 {
    let from = mv.from as usize;
    let to = mv.to as usize;
    let piece = b.sq[from].abs();
    let own = b.side_idx();
    let opp = 1 - own;
    let mass = if piece == KING { 4.0 } else { value(piece) };
    let captured = if mv.flag == FLAG_EP {
        1.0
    } else {
        value(b.sq[to])
    };
    let capture = (captured > 0.0) as u8 as f32;
    let pawn = (piece == PAWN) as u8 as f32;
    let minor = (piece == KNIGHT || piece == BISHOP) as u8 as f32;
    let a = w.amp[opp][from];
    let d = w.amp[own][from];
    let t = w.amp[opp][to];
    let u = w.amp[own][to];
    let advance = if b.side > 0 {
        rank_of(to)
    } else {
        7 - rank_of(to)
    } as f32;
    let centre_gain = centre(to) - centre(from);
    let their_king = b.ksq[opp] as usize;
    let step = distance(from, to);
    let check = check as u8 as f32;
    let free = (t == 0.0) as u8 as f32;
    // Each piece has an intercept and 35 coefficients. Selecting a row is
    // sufficient; there are no hidden layers or evaluations of other pieces.
    let terms = [
        1.0,
        captured,
        value(mv.promo as i8),
        matches!(mv.flag, FLAG_CASTLE_K | FLAG_CASTLE_Q) as u8 as f32,
        check,
        advance,
        ((rank_of(to) - rank_of(from)) * b.side) as f32,
        centre(to),
        centre_gain,
        distance(to, their_king),
        distance(from, their_king) - distance(to, their_king),
        distance(to, b.ksq[own] as usize),
        step,
        a,
        d,
        t,
        u,
        w.latent[opp][to],
        w.latent[own][to],
        w.hits[opp][to] as f32,
        w.hits[own][to] as f32,
        (w.pin[from] != 0) as u8 as f32,
        (w.pin[to] != 0) as u8 as f32,
        mass * a / (1.0 + d),
        mass * t / (1.0 + u),
        capture,
        captured * free,
        (captured - mass).min(0.0) * (1.0 - free),
        mass * ((a > 0.0 && d == 0.0) as u8 as f32),
        capture * free,
        pawn * advance * advance / 8.0,
        minor * centre_gain,
        (piece == KING) as u8 as f32 * centre(to),
        (piece == QUEEN) as u8 as f32 * step,
        (piece == ROOK) as u8 as f32 * advance,
        check * free,
    ];
    terms
        .iter()
        .zip(&WEIGHTS[(piece - 1) as usize])
        .map(|(x, w)| x * w)
        .sum()
}

pub(crate) struct Transition {
    pub mv: Move,
    pub board: Board,
    pub preference: f32,
    pub check: bool,
    protected: bool,
}

/// Generate the child board once for both the policy and the full wave evaluation.
pub(crate) fn transitions(b: &Board, game: &[u64], chain: &[u64]) -> Vec<Transition> {
    let mut pseudo = MoveList::new();
    b.gen_pseudo(&mut pseudo, false);
    let field = WaveField::compute_with(b, &Ps::load());
    let mut moves = Vec::with_capacity(pseudo.n);
    let own = b.side_idx();
    let checks = crate::checks::CheckMap::new(b, field.raw_hits[own][b.ksq[1 - own] as usize] != 0);
    let in_check = field.raw_hits[1 - own][b.ksq[own] as usize] != 0;
    for &mv in &pseudo.m[..pseudo.n] {
        let from = mv.from as usize;
        let after = b.make(mv);
        if (in_check || from == b.ksq[own] as usize || field.pin[from] != 0 || mv.flag == FLAG_EP)
            && after.attacked(after.ksq[own] as usize, -b.side)
        {
            continue;
        }
        let check = checks.possible(b, mv) && after.in_check();
        let preference = preference(b, &field, mv, check);
        let protected = check
            || mv.promo != 0
            || mv.flag == FLAG_EP
            || b.sq[mv.to as usize] != 0
            || game.contains(&after.hash)
            || chain.contains(&after.hash);
        moves.push(Transition {
            mv,
            board: after,
            preference,
            check,
            protected,
        });
    }
    moves.sort_by(|a, b| b.preference.total_cmp(&a.preference));
    let top = moves.len().min(4);
    let reference = moves[..top].iter().map(|t| t.preference).sum::<f32>() / top.max(1) as f32;
    for t in &mut moves {
        t.preference = (t.preference - reference).clamp(-4.0, 3.0);
    }
    // Do not restrict the root, evasions or imminent fifty-move draws.
    if !chain.is_empty() && !in_check && b.halfmove < 99 {
        let mut quiet = 0;
        moves.retain(|t| {
            if t.protected {
                return true;
            }
            quiet += 1;
            quiet <= 5
        });
    }
    moves
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_transitions_match_legal_moves_and_colour_reflection() {
        for fen in [
            Board::START_FEN,
            "r3k2r/1P6/8/3pP3/8/8/8/R3K2R w KQkq d6 0 15",
            "k3r3/8/8/8/8/8/4N3/4K3 w - - 0 1",
        ] {
            let b = Board::from_fen(fen).unwrap();
            let mirror = b.mirrored();
            let moves = transitions(&b, &[], &[]);
            let others = transitions(&mirror, &[], &[]);
            let legal = b.legal_moves();
            assert_eq!(moves.len(), legal.n);
            for t in moves {
                assert!(legal.m[..legal.n].contains(&t.mv));
                let other = others
                    .iter()
                    .find(|o| {
                        o.mv.from == t.mv.from ^ 56
                            && o.mv.to == t.mv.to ^ 56
                            && o.mv.promo == t.mv.promo
                    })
                    .unwrap();
                assert!((t.preference - other.preference).abs() < 0.001);
            }
        }
    }

    #[test]
    fn deep_wave_keeps_forcing_moves_and_draw_history() {
        let b = Board::from_fen("r3k2r/1P6/8/3pP3/8/8/8/R3K2R w KQkq d6 0 15").unwrap();
        let full = transitions(&b, &[], &[]);
        let compact = transitions(&b, &[], &[b.hash]);
        assert_eq!(compact.iter().filter(|t| !t.protected).count(), 5);
        for t in full.iter().filter(|t| t.protected) {
            assert!(compact.iter().any(|c| c.mv == t.mv));
        }
        let omitted = full
            .iter()
            .find(|t| !compact.iter().any(|c| c.mv == t.mv))
            .unwrap();
        let history = transitions(&b, &[omitted.board.hash], &[b.hash]);
        assert!(history.iter().any(|c| c.mv == omitted.mv));
        let checked = Board::from_fen("4k3/8/8/8/2b5/8/4r3/4K3 w - - 0 1").unwrap();
        assert_eq!(
            transitions(&checked, &[], &[checked.hash]).len(),
            checked.legal_moves().n
        );
    }
}
