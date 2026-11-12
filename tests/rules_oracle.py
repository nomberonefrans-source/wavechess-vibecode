#!/usr/bin/env python3
"""Cross-check chess rules against python-chess; requires rustc and python-chess.

Runs deterministic random legal positions plus en-passant pin regressions.
The independently compiled helper uses the engine's board and geometry modules.
"""
import json
import random
import subprocess
import tempfile
from pathlib import Path

import chess

ROOT = Path(__file__).resolve().parents[1]
RUST = r'''
#[path = "BOARD"] mod board;
#[path = "GEOMETRY"] mod geometry;
use std::io::{self, BufRead};
fn main() {
    for line in io::stdin().lock().lines() {
        let b = board::Board::from_fen(&line.unwrap()).unwrap();
        let list = b.legal_moves();
        let mut moves: Vec<_> = list.m[..list.n].iter().map(|m| {
            let next = b.make(*m);
            assert_eq!(next.hash, next.compute_hash());
            m.to_uci()
        }).collect();
        moves.sort();
        println!("{}|{}|{}", moves.join(" "), b.perft(2), b.insufficient());
    }
}
'''


def main():
    rng = random.Random(22026)
    positions = []
    for _ in range(10):
        board = chess.Board()
        for ply in range(100):
            legal = list(board.legal_moves)
            if not legal:
                break
            board.push(rng.choice(legal))
            if ply % 4 == 0:
                positions.append(board.copy())
    for fen in [
        "4k3/8/8/3p4/8/8/8/4K3 w - d6 0 1",
        "k3r3/8/8/3pP3/8/8/8/4K3 w - d6 0 1",
        "4k3/8/8/r4pPK/8/8/8/8 w - f6 0 1",
    ]:
        positions.append(chess.Board(fen))
    with tempfile.TemporaryDirectory(prefix="wavechess_rules_") as directory:
        source = Path(directory) / "oracle.rs"
        executable = Path(directory) / "oracle"
        code = RUST.replace('"BOARD"', json.dumps(str(ROOT / "src/board.rs")))
        code = code.replace('"GEOMETRY"', json.dumps(str(ROOT / "src/geometry.rs")))
        source.write_text(code)
        subprocess.run(["rustc", "--edition=2021", "-Awarnings", "-O", str(source), "-o", str(executable)], check=True)
        fens = "".join(b.fen(en_passant="fen") + "\n" for b in positions)
        result = subprocess.run([str(executable)], input=fens, text=True, capture_output=True, check=True)
    total_nodes = 0
    for board, line in zip(positions, result.stdout.splitlines(), strict=True):
        moves, count, dead = line.split("|")
        assert moves.split() == sorted(m.uci() for m in board.legal_moves), board.fen()
        nodes = 0
        for move in list(board.legal_moves):
            board.push(move)
            nodes += board.legal_moves.count()
            board.pop()
        assert int(count) == nodes, (board.fen(), count, nodes)
        assert (dead == "true") == board.is_insufficient_material(), board.fen()
        total_nodes += nodes
    print(json.dumps({"positions": len(positions), "perft2_nodes": total_nodes,
                      "seed": 22026, "legal_moves_match": True, "perft2_match": True,
                      "incremental_hashes_match": True, "reference": f"python-chess {chess.__version__}"}, indent=2))


if __name__ == "__main__":
    main()
