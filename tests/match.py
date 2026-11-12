#!/usr/bin/env python3
"""Small paired match, no strength adjudication or Elo inference. Requires python-chess."""
import argparse
import contextlib
import json
from pathlib import Path
import chess
import chess.engine
import chess.pgn

OPENINGS = [
    [],
    ['e2e4', 'e7e5', 'g1f3', 'b8c6', 'f1c4', 'g8f6'],
    ['d2d4', 'd7d5', 'c2c4', 'e7e6', 'b1c3', 'g8f6'],
    ['e2e4', 'c7c5', 'g1f3', 'd7d6', 'd2d4', 'c5d4', 'f3d4', 'g8f6'],
]

def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--baseline', required=True)
    ap.add_argument('--revised', required=True)
    ap.add_argument('--movetime-ms', type=int, default=40)
    ap.add_argument('--max-plies', type=int, default=240)
    ap.add_argument('--output', type=Path, required=True)
    args = ap.parse_args()
    rows, pgns = [], []
    with contextlib.ExitStack() as stack:
        engines = {name: stack.enter_context(chess.engine.SimpleEngine.popen_uci([str(Path(path).resolve()), 'uci'], timeout=30))
                   for name, path in [('baseline', args.baseline), ('revised', args.revised)]}
        for engine in engines.values():
            engine.configure({'Threads': 1, 'Hash': 16, 'Move Overhead': 0})
        for oi, opening in enumerate(OPENINGS):
            for revised_white in (True, False):
                sides = {chess.WHITE: 'revised' if revised_white else 'baseline',
                         chess.BLACK: 'baseline' if revised_white else 'revised'}
                board = chess.Board()
                for move in opening:
                    board.push_uci(move)
                game_id = object()
                for engine in engines.values():
                    engine.configure({'Clear Hash': None})
                while not board.is_game_over(claim_draw=True) and len(board.move_stack) < args.max_plies:
                    reply = engines[sides[board.turn]].play(board, chess.engine.Limit(time=args.movetime_ms / 1000, depth=60), game=game_id)
                    assert reply.move in board.legal_moves, (sides, board.fen(), reply.move)
                    board.push(reply.move)
                outcome = board.outcome(claim_draw=True)
                result = outcome.result() if outcome else '*'
                winner = None if outcome is None or outcome.winner is None else sides[outcome.winner]
                row = {'opening': oi, 'revised_white': revised_white, 'result': result, 'winner': winner,
                       'termination': outcome.termination.name if outcome else 'PLY_LIMIT_UNFINISHED',
                       'plies': len(board.move_stack), 'final_fen': board.fen()}
                rows.append(row)
                game = chess.pgn.Game.from_board(board)
                game.headers.update({'Event': 'WaveChess paired smoke match', 'White': sides[chess.WHITE],
                                     'Black': sides[chess.BLACK], 'Result': result, 'Termination': row['termination']})
                pgns.append(str(game))
                args.output.write_text(json.dumps({'movetime_ms': args.movetime_ms, 'max_plies': args.max_plies,
                    'threads': 1, 'cache_mb': 16, 'games': rows}, indent=2) + '\n')
                args.output.with_suffix('.pgn').write_text('\n\n'.join(pgns) + '\n')
                print(row, flush=True)

if __name__ == '__main__':
    main()
