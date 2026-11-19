//! A forward-wave prior. All coordinates are relative to the mover.
//! Direct contacts and a slider's first screened target are kept separate.
use crate::board::{Board, Move, MoveList, FLAG_CASTLE_K, FLAG_CASTLE_Q, FLAG_EP};
use crate::geometry::*;
use crate::params::Ps;
use crate::wave::{WaveField, MASS};

const FEATURES: usize = 43;
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

#[derive(Clone, Copy, Default)]
pub(crate) struct Contacts {
    threat: f32,
    second: f32,
    support: f32,
    screened: f32,
    reach: f32,
}

impl Contacts {
    fn visit(&mut self, b: &Board, s: usize, side: i8, screened: bool) {
        let piece = b.sq[s];
        if screened {
            if piece.signum() == -side {
                self.screened = self.screened.max(value(piece));
            }
        } else if piece.signum() == side {
            self.support += value(piece);
        } else {
            self.reach += 1.0;
            let mass = value(piece);
            if mass >= self.threat {
                self.second = self.threat;
                self.threat = mass;
            } else {
                self.second = self.second.max(mass);
            }
        }
    }
}

pub(crate) fn contacts(b: &Board, square: usize) -> Contacts {
    let p = b.sq[square];
    let side = p.signum();
    let mut result = Contacts::default();
    let pawn_directions = [(-1, side), (1, side)];
    let leaps = if p.abs() == PAWN {
        &pawn_directions[..]
    } else {
        leaper_steps(p.abs())
    };
    let f = file_of(square);
    let r = rank_of(square);
    for &(df, dr) in leaps {
        if inside(f + df, r + dr) {
            result.visit(b, sq_of(f + df, r + dr), side, false);
        }
    }
    for &(df, dr) in slider_rays(p.abs()) {
        let (mut nf, mut nr) = (f + df, r + dr);
        let mut screened = false;
        while inside(nf, nr) {
            let s = sq_of(nf, nr);
            result.visit(b, s, side, screened);
            if b.sq[s] != 0 {
                if screened {
                    break;
                }
                screened = true;
            }
            nf += df;
            nr += dr;
        }
    }
    result
}

pub(crate) fn features(
    b: &Board,
    w: &WaveField,
    mv: Move,
    next: &Board,
    origin: Contacts,
    check: bool,
) -> [f32; FEATURES] {
    let from = mv.from as usize;
    let to = mv.to as usize;
    let piece = b.sq[from];
    let own = b.side_idx();
    let opp = 1 - own;
    let their_king = b.ksq[opp] as usize;
    let our_king = b.ksq[own] as usize;
    let target = contacts(next, to);
    let advance = |s| if b.side > 0 { rank_of(s) } else { 7 - rank_of(s) } as f32;
    let captured = if mv.flag == FLAG_EP {
        1.0
    } else {
        value(b.sq[to])
    };
    [
        (piece.abs() == PAWN) as u8 as f32,
        (piece.abs() == KNIGHT) as u8 as f32,
        (piece.abs() == BISHOP) as u8 as f32,
        (piece.abs() == ROOK) as u8 as f32,
        (piece.abs() == QUEEN) as u8 as f32,
        (piece.abs() == KING) as u8 as f32,
        captured,
        value(mv.promo as i8),
        matches!(mv.flag, FLAG_CASTLE_K | FLAG_CASTLE_Q) as u8 as f32,
        check as u8 as f32,
        advance(to),
        advance(to) - advance(from),
        centre(to),
        centre(to) - centre(from),
        distance(to, their_king),
        distance(from, their_king) - distance(to, their_king),
        distance(to, our_king),
        distance(from, to),
        w.amp[opp][from],
        w.amp[own][from],
        w.amp[opp][to],
        w.amp[own][to],
        w.latent[opp][from],
        w.latent[own][from],
        w.latent[opp][to],
        w.latent[own][to],
        w.hits[opp][to] as f32,
        w.hits[own][to] as f32,
        (w.pin[from] != 0) as u8 as f32,
        (w.pin[to] != 0) as u8 as f32,
        target.threat,
        target.second,
        target.support,
        target.screened,
        target.reach,
        target.threat - origin.threat,
        target.second - origin.second,
        target.support - origin.support,
        target.screened - origin.screened,
        target.reach - origin.reach,
        value(piece) * w.amp[opp][from],
        value(piece) * w.amp[opp][to],
        captured - value(piece) * w.hits[opp][to].min(1) as f32,
    ]
}

pub(crate) fn logit(x: &[f32; FEATURES]) -> f32 {
    let mut layer = BIAS;
    for (input, weights) in x.iter().zip(WEIGHTS.iter()) {
        for (sum, weight) in layer.iter_mut().zip(weights) {
            *sum += input * weight;
        }
    }
    layer.iter().zip(OUTPUT).map(|(&v, w)| v.max(0.0) * w).sum()
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
    let mut origins = [None; 64];
    let mut moves = Vec::with_capacity(pseudo.n);
    let own = b.side_idx();
    let in_check = field.raw_hits[1 - own][b.ksq[own] as usize] != 0;
    for &mv in &pseudo.m[..pseudo.n] {
        let from = mv.from as usize;
        let after = b.make(mv);
        if after.attacked(after.ksq[own] as usize, -b.side) {
            continue;
        }
        let origin = *origins[from].get_or_insert_with(|| contacts(b, from));
        let check = after.in_check();
        let preference = logit(&features(b, &field, mv, &after, origin, check));
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
