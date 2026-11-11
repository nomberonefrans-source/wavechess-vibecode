#!/usr/bin/env python3
"""Reproducible UCI tactical/speed comparison; requires python-chess.

Usage: python tests/compare.py --engine baseline=/path/to/old --engine revised=/path/to/new --output results.json
A small regression suite measures these positions only; it is not an Elo test.
"""
import argparse
import json
import statistics
import time
from pathlib import Path

import chess
import chess.engine

CASES = [
    ("start", chess.STARTING_FEN, None, False),
    ("kiwipete", "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1", None, False),
    ("middlegame", "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10", None, False),
    ("mate_in_one", "6k1/5ppp/8/8/8/8/8/R5K1 w - - 0 1", {"a1a8"}, True),
    ("hanging_queen", "rnb1kbnr/pppp1ppp/8/4p3/4P2q/5N2/PPPP1PPP/RNBQKB1R w KQkq - 2 3", {"f3h4"}, False),
    ("back_rank_defense", "4r1k1/5ppp/8/8/8/8/5PPP/R5K1 w - - 0 1", None, True),
    ("queen_sacrifice", "5r1k/6pp/7N/8/8/1Q6/8/6K1 w - - 0 1", {"b3g8"}, False),
    ("no_false_sacrifice", "5r1k/6pp/8/8/8/1Q6/8/6K1 w - - 0 1", None, False),
    ("quiet_mating_move", "6k1/5ppp/4n3/2N2RQ1/8/6K1/8/8 w - - 0 1", {"c5e6"}, False),
    ("defend_mating_net", "5r1k/6pp/7N/8/8/1Q6/8/6K1 b - - 0 1", {"f8f1", "g7h6"}, False),
    ("ep_endgame", "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1", None, False),
    ("promotion_tactics", "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8", None, False),
]


def inspect(board, info, expected, safety):
    pv = info.get("pv", [])
    legal = bool(pv)
    probe = board.copy()
    for move in pv:
        if move not in probe.legal_moves:
            legal = False
            break
        probe.push(move)
    move = pv[0] if pv else chess.Move.null()
    safe = True
    if legal:
        after = board.copy()
        after.push(move)
        for reply in list(after.legal_moves):
            after.push(reply)
            if after.is_checkmate():
                safe = False
            after.pop()
    score = info.get("score")
    return {
        "bestmove": move.uci(),
        "pv": [m.uci() for m in pv],
        "legal_pv": legal,
        "safe_from_mate_in_one": safe,
        "score": str(score.pov(board.turn)) if score else None,
        "depth": info.get("depth"),
        "nodes": info.get("nodes"),
        "engine_time_ms": round(1000 * info.get("time", 0)),
        "tactical_pass": (move.uci() in expected if expected else safe) if expected or safety else None,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", action="append", required=True, help="LABEL=/absolute/path/to/executable")
    parser.add_argument("--depth", type=int, default=5)
    parser.add_argument("--movetime-ms", type=int, default=250)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    results = {"depth": args.depth, "movetime_ms": args.movetime_ms, "threads": 1, "cache_mb": 16, "engines": {}}
    for entry in args.engine:
        label, executable = entry.split("=", 1)
        print(label, flush=True)
        engine = chess.engine.SimpleEngine.popen_uci([str(Path(executable).resolve()), "uci"], timeout=120)
        engine.configure({"Threads": 1, "Hash": 16, "Move Overhead": 0})
        rows = []
        try:
            for mode in ("fixed_depth", "equal_time"):
                for name, fen, expected, safety in CASES:
                    board = chess.Board(fen)
                    assert board.is_valid(), (name, board.status())
                    engine.configure({"Clear Hash": None})
                    limit = chess.engine.Limit(depth=args.depth) if mode == "fixed_depth" else chess.engine.Limit(depth=60, time=args.movetime_ms / 1000)
                    start = time.perf_counter()
                    info = engine.analyse(board, limit, game=object())
                    elapsed = (time.perf_counter() - start) * 1000
                    row = {"case": name, "mode": mode, "wall_ms": round(elapsed, 2), **inspect(board, info, expected, safety)}
                    rows.append(row)
                    print(mode, name, row["bestmove"], row["depth"], row["wall_ms"], row["tactical_pass"], flush=True)
        finally:
            engine.quit()
        results["engines"][label] = {"cases": rows, "summary": {}}
        for mode in ("fixed_depth", "equal_time"):
            part = [r for r in rows if r["mode"] == mode]
            judged = [r for r in part if r["tactical_pass"] is not None]
            results["engines"][label]["summary"][mode] = {
                "tactical_passes": sum(r["tactical_pass"] for r in judged),
                "tactical_cases": len(judged),
                "legal_pvs": sum(r["legal_pv"] for r in part),
                "total_wall_ms": round(sum(r["wall_ms"] for r in part), 2),
                "median_depth": statistics.median(r["depth"] or 0 for r in part),
            }
        args.output.write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
