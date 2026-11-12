#!/usr/bin/env python3
"""Paired UCI matches. No evaluation adjudication; unfinished games stay unfinished.

Requires python-chess. Both engines receive the same limits. Strength options,
executable hashes, final positions and every move are recorded alongside results.
"""
import argparse
import contextlib
import hashlib
import json
import statistics
import time
from pathlib import Path

import chess
import chess.engine
import chess.pgn

HOLDOUT = [
    ['e2e4', 'e7e5', 'g1f3', 'b8c6', 'f1b5', 'a7a6'],
    ['d2d4', 'g8f6', 'c2c4', 'g7g6', 'b1c3', 'f8g7'],
    ['e2e4', 'e7e6', 'd2d4', 'd7d5', 'b1c3', 'g8f6'],
    ['e2e4', 'c7c6', 'd2d4', 'd7d5', 'b1c3', 'd5e4'],
    ['c2c4', 'e7e5', 'b1c3', 'g8f6', 'g2g3', 'd7d5'],
    ['g1f3', 'd7d5', 'g2g3', 'g8f6', 'f1g2', 'c7c5'],
    ['d2d4', 'd7d5', 'c2c4', 'c7c6', 'g1f3', 'g8f6'],
    ['e2e4', 'e7e5', 'b1c3', 'g8f6', 'f1c4', 'b8c6'],
    ['e2e4', 'e7e5', 'g1f3', 'b8c6', 'd2d4', 'e5d4'],
    ['e2e4', 'd7d5', 'e4d5', 'd8d5', 'b1c3', 'd5a5'],
    ['e2e4', 'd7d6', 'd2d4', 'g8f6', 'b1c3', 'g7g6'],
    ['d2d4', 'd7d5', 'c2c4', 'd5c4', 'g1f3', 'g8f6'],
    ['d2d4', 'f7f5', 'g2g3', 'g8f6', 'f1g2', 'e7e6'],
    ['e2e4', 'e7e5', 'f2f4', 'e5f4', 'g1f3', 'g7g5'],
    ['g1f3', 'g8f6', 'c2c4', 'e7e6', 'b1c3', 'b7b6'],
    ['f2f4', 'd7d5', 'g1f3', 'g8f6', 'e2e3', 'c7c5'],
]
# These deliberately winning starting positions test conversion only. Results
# must never be described as beating Stockfish from an equal initial position.
ENDGAMES = [
    '8/8/8/8/3k4/8/8/1Q2K3 w - - 0 1',
    '8/8/8/8/3k4/8/8/R3K3 w - - 0 1',
    '6k1/5ppp/8/8/8/8/5PPP/3Q2K1 w - - 0 1',
    '6k1/5ppp/8/8/8/8/5PPP/R4RK1 w - - 0 1',
]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--baseline', required=True)
    p.add_argument('--revised', required=True)
    p.add_argument('--baseline-options', default='{}', help='JSON UCI options')
    p.add_argument('--revised-options', default='{}', help='JSON UCI options')
    p.add_argument('--movetime-ms', type=int, default=100)
    p.add_argument('--nodes', type=int, default=0)
    p.add_argument('--depth', type=int, default=64)
    p.add_argument('--games', type=int, default=16)
    p.add_argument('--opening-offset', type=int, default=0)
    p.add_argument('--max-plies', type=int, default=300)
    p.add_argument('--suite', choices=['holdout', 'start', 'conversion'], default='holdout')
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    if args.games < 1 or args.movetime_ms < 1 or args.max_plies < 1:
        p.error('games, movetime-ms and max-plies must be positive')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    data = {'suite': args.suite, 'opening_offset': args.opening_offset, 'movetime_ms': args.movetime_ms, 'nodes': args.nodes,
            'depth_limit': args.depth, 'max_plies': args.max_plies,
            'draw_claims': True, 'engines': {}, 'games': []}
    pgns = []
    with contextlib.ExitStack() as stack:
        engines = {}
        for label, path, options in [('baseline', args.baseline, args.baseline_options),
                                     ('revised', args.revised, args.revised_options)]:
            exe = Path(path).resolve()
            engine = stack.enter_context(chess.engine.SimpleEngine.popen_uci([str(exe)], timeout=60))
            cfg = {'Threads': 1, 'Hash': 16, 'Move Overhead': 0}
            if 'Opening Variety' in engine.options:
                cfg.update({'Opening Variety': 0, 'Random Seed': 42})
            cfg.update(json.loads(options))
            engine.configure(cfg)
            engines[label] = engine
            data['engines'][label] = {'name': engine.id, 'executable_sha256': hashlib.sha256(exe.read_bytes()).hexdigest(),
                                     'options': cfg}
        for i in range(args.games):
            pair = i // 2
            revised_white = i % 2 == 0
            if args.suite == 'conversion':
                board = chess.Board(ENDGAMES[pair % len(ENDGAMES)])
                if not revised_white:
                    board = board.mirror()
            else:
                board = chess.Board()
                if args.suite == 'holdout':
                    for move in HOLDOUT[(pair + args.opening_offset) % len(HOLDOUT)]:
                        board.push_uci(move)
            assert board.is_valid(), board.fen()
            initial_fen = board.fen()
            played = 0
            sides = {chess.WHITE: 'revised' if revised_white else 'baseline',
                     chess.BLACK: 'baseline' if revised_white else 'revised'}
            game_id = object()
            elapsed = {label: 0.0 for label in engines}
            depths = {label: [] for label in engines}
            while not board.is_game_over(claim_draw=True) and played < args.max_plies:
                label = sides[board.turn]
                limit = chess.engine.Limit(nodes=args.nodes, depth=args.depth) if args.nodes else chess.engine.Limit(time=args.movetime_ms / 1000, depth=args.depth)
                start = time.perf_counter()
                result = engines[label].play(board, limit, game=game_id, info=chess.engine.INFO_BASIC)
                elapsed[label] += time.perf_counter() - start
                assert result.move in board.legal_moves, (label, board.fen(), result.move)
                depths[label].append(result.info.get('depth', 0))
                board.push(result.move)
                played += 1
            outcome = board.outcome(claim_draw=True)
            row = {'game': i + 1, 'pair': pair, 'revised_white': revised_white,
                   'initial_fen': initial_fen, 'final_fen': board.fen(),
                   'result': outcome.result() if outcome else '*',
                   'winner': sides[outcome.winner] if outcome and outcome.winner is not None else None,
                   'termination': outcome.termination.name if outcome else 'PLY_LIMIT_UNFINISHED',
                   'played_plies': played, 'wall_seconds': elapsed,
                   'mean_reported_depth': {label: statistics.mean(values) if values else 0 for label, values in depths.items()}}
            data['games'].append(row)
            g = chess.pgn.Game.from_board(board)
            g.headers.update({'Event': 'WaveChess ' + args.suite, 'White': sides[chess.WHITE],
                              'Black': sides[chess.BLACK], 'Result': row['result'],
                              'Termination': row['termination'], 'TimeControl': f'{args.movetime_ms}ms/move'})
            pgns.append(str(g))
            rows = data['games']
            data['summary'] = {'revised_wins': sum(r['winner'] == 'revised' for r in rows),
                               'baseline_wins': sum(r['winner'] == 'baseline' for r in rows),
                               'draws': sum(r['result'] == '1/2-1/2' for r in rows),
                               'unfinished': sum(r['result'] == '*' for r in rows)}
            args.output.write_text(json.dumps(data, indent=2) + '\n')
            args.output.with_suffix('.pgn').write_text('\n\n'.join(pgns) + '\n')
            print(json.dumps(row), flush=True)
    print(json.dumps(data['summary']), flush=True)


if __name__ == '__main__':
    main()
