#!/usr/bin/env python3
"""Audit a deterministic, unfiltered sample of game positions with Stockfish.

This is a finite-budget external oracle, not an Elo test or proof of correctness.
Selection uses only game/ply indices, never the engine's errors or evaluations.
"""
import argparse
import contextlib
import hashlib
import json
import statistics
from pathlib import Path

import chess
import chess.engine
import chess.pgn
from quick_elo import start_engine


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--engine', action='append', required=True, help='label=path')
    p.add_argument('--stockfish', required=True)
    p.add_argument('--pgn', type=Path, required=True)
    p.add_argument('--positions', type=int, default=64)
    p.add_argument('--oracle-nodes', type=int, default=30000)
    p.add_argument('--movetime-ms', type=int, default=120)
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    if a.output.exists():
        p.error('output already exists')
    positions = []
    with a.pgn.open() as stream:
        game_index = 0
        while len(positions) < a.positions:
            game = chess.pgn.read_game(stream)
            if game is None:
                break
            assert not game.errors
            game_index += 1
            board = game.board()
            for ply, move in enumerate(game.mainline_moves(), 1):
                board.push(move)
                if ply in (16, 28, 40, 52) and not board.is_game_over(claim_draw=True):
                    positions.append((game_index, ply, board.copy()))
                    if len(positions) == a.positions:
                        break
    data = {'selection': 'first games, plies 16/28/40/52, no evaluation filtering',
            'source_sha256': hashlib.sha256(a.pgn.read_bytes()).hexdigest(),
            'oracle_nodes_per_analysis': a.oracle_nodes, 'movetime_ms': a.movetime_ms,
            'mate_score_for_loss_cp': 10000, 'rows': [], 'engines': {}}
    with contextlib.ExitStack() as stack:
        oracle, data['oracle'] = start_engine(stack, a.stockfish, {})
        assert oracle.id['name'] == 'Stockfish 19'
        engines = {}
        for spec in a.engine:
            label, path = spec.split('=', 1)
            engines[label], data['engines'][label] = start_engine(stack, path, {})
        for game_index, ply, board in positions:
            best = oracle.analyse(board, chess.engine.Limit(nodes=a.oracle_nodes), game=object())
            best_cp = best['score'].pov(board.turn).score(mate_score=10000)
            selected = {}
            for label, engine in engines.items():
                answer = engine.analyse(board, chess.engine.Limit(time=a.movetime_ms/1000), game=object())
                selected[label] = answer['pv'][0]
            evaluated = {best['pv'][0]: best_cp}
            for move in set(selected.values()) - evaluated.keys():
                value = oracle.analyse(board, chess.engine.Limit(nodes=a.oracle_nodes),
                                       root_moves=[move], game=object())
                evaluated[move] = value['score'].pov(board.turn).score(mate_score=10000)
            # Equal finite node budgets can disagree; retain raw values and use
            # the strongest observed reference rather than negative move loss.
            reference = max(best_cp, *evaluated.values())
            row = {'game': game_index, 'ply': ply, 'fen': board.fen(),
                   'oracle_best': best['pv'][0].uci(), 'oracle_cp': best_cp, 'moves': {}}
            for label, move in selected.items():
                after = board.copy()
                after.push(move)
                allows_mate = any(after.gives_check(reply) and _mates(after, reply)
                                  for reply in after.legal_moves)
                row['moves'][label] = {'move': move.uci(), 'oracle_cp': evaluated[move],
                    'loss_cp': reference - evaluated[move], 'allows_mate_in_one': allows_mate}
            avoidable = (any(m['allows_mate_in_one'] for m in row['moves'].values())
                         and any(not _allows_mate(board, move) for move in board.legal_moves))
            for move in row['moves'].values():
                move['avoidable_mate_in_one'] = move['allows_mate_in_one'] and avoidable
            data['rows'].append(row)
            a.output.write_text(json.dumps(data, indent=2) + '\n')
            print(json.dumps({'position': len(data['rows']), 'moves': row['moves']}), flush=True)
    data['summary'] = {}
    for label in data['engines']:
        moves = [r['moves'][label] for r in data['rows']]
        data['summary'][label] = {'positions': len(moves),
            'mean_loss_cp': round(statistics.mean(m['loss_cp'] for m in moves), 2),
            'median_loss_cp': statistics.median(m['loss_cp'] for m in moves),
            'loss_at_least_200cp': sum(m['loss_cp'] >= 200 for m in moves),
            'loss_at_least_100cp': sum(m['loss_cp'] >= 100 for m in moves),
            'allows_mate_in_one': sum(m['allows_mate_in_one'] for m in moves),
            'avoidable_mate_in_one': sum(m['avoidable_mate_in_one'] for m in moves)}
    a.output.write_text(json.dumps(data, indent=2) + '\n')
    print(json.dumps(data['summary'], indent=2))


def _mates(board, move):
    child = board.copy()
    child.push(move)
    return child.is_checkmate()


def _allows_mate(board, move):
    child = board.copy()
    child.push(move)
    return any(_mates(child, reply) for reply in child.legal_moves)


if __name__ == '__main__':
    main()
