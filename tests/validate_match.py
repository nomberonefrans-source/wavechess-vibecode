#!/usr/bin/env python3
"""Independently replay all PGNs and verify arena JSON outcomes and pairing."""
import argparse
import json
from pathlib import Path

import chess.pgn
from quick_elo import rating_summary


def validate(path):
    data = json.loads(path.read_text())
    with path.with_suffix('.pgn').open() as stream:
        for row in data['games']:
            game = chess.pgn.read_game(stream)
            assert game is not None and not game.errors, row['game']
            board = game.end().board()
            assert board.fen() == row['final_fen'], row['game']
            outcome = board.outcome(claim_draw=True)
            result = outcome.result() if outcome else '*'
            score = None if outcome is None else (0.5 if outcome.winner is None else
                    float(outcome.winner == row['candidate_white']))
            assert row['result'] == game.headers['Result'] == result, row['game']
            assert row['score'] == score, row['game']
            assert row['error'] is None, row['game']
            assert int(game.headers['NominalOpponentElo']) == row['anchor']
            if outcome:
                assert row['termination'] == outcome.termination.name
        assert chess.pgn.read_game(stream) is None
    assert data['summary']['rating'] == rating_summary(data['games'])
    return {'file': str(path), 'games': len(data['games']), 'pgn_results_verified': True,
            'rating_recomputed': True, 'candidate_sha256': data['candidate']['sha256']}


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('files', nargs='+', type=Path)
    print(json.dumps([validate(path) for path in p.parse_args().files], indent=2))
