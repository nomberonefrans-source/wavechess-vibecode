"""Measure the first six full moves against a deterministic opponent."""
import argparse
import collections
import json
from pathlib import Path

import chess
import chess.engine


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--opponent', type=Path, required=True)
    parser.add_argument('--games', type=int, default=100)
    parser.add_argument('--depth', type=int, default=1)
    args = parser.parse_args()
    openings = collections.Counter()
    limit = chess.engine.Limit(depth=args.depth)
    with chess.engine.SimpleEngine.popen_uci(str(args.engine.resolve())) as engine, chess.engine.SimpleEngine.popen_uci(str(args.opponent.resolve())) as fixed:
        fixed.configure({'Threads': 1, 'Hash': 1, 'Opening Variety': 0})
        for _ in range(args.games):
            game = object()
            board = chess.Board()
            for ply in range(12):
                if board.is_game_over(claim_draw=True):
                    break
                player = engine if board.turn else fixed
                move = player.play(board, limit, game=game).move
                if move not in board.legal_moves:
                    raise ValueError(f'Illegal move {move} in {board.fen()}')
                board.push(move)
            openings[' '.join(m.uci() for m in board.move_stack)] += 1
    print(json.dumps({'games': args.games, 'depth': args.depth,
                      'distinct': len(openings), 'sequences': openings}, indent=2))


if __name__ == '__main__':
    main()
