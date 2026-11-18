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


def geometry(board):
    """Presence of a slider ray through an enemy unit to another enemy unit."""
    flags = set()
    mass = (0, 1, 3, 3, 5, 9, 99)
    for side in chess.COLORS:
        enemy = board.occupied_co[not side]
        for src in chess.scan_forward((board.bishops | board.rooks | board.queens) & board.occupied_co[side]):
            for front in chess.scan_forward(board.attacks_mask(src) & enemy):
                beyond = chess.ray(src, front) & enemy
                for rear in chess.scan_forward(beyond):
                    if chess.between(src, rear) & board.occupied == chess.BB_SQUARES[front]:
                        flags.add('xray')
                        if board.piece_type_at(rear) == chess.KING:
                            flags.add('absolute_pin')
                        if mass[board.piece_type_at(front)] > mass[board.piece_type_at(rear)]:
                            flags.add('skewer_geometry')
    return sorted(flags)


def band(cp):
    return 'win' if cp >= 150 else 'loss' if cp <= -150 else 'equal'


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--engine', action='append', required=True, help='label=path')
    p.add_argument('--stockfish', required=True)
    p.add_argument('--pgn', type=Path, required=True)
    p.add_argument('--positions', type=int, default=1000)
    p.add_argument('--oracle-nodes', type=int, default=30000)
    p.add_argument('--movetime-ms', type=int, default=120)
    p.add_argument('--wave-nodes', type=int, default=0)
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    if a.output.exists():
        p.error('output already exists')
    positions = []
    seen = set()
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
                if ply >= 12 and ply % 4 == 0 and not board.is_game_over(claim_draw=True):
                    key = board.fen().rsplit(' ', 2)[0]
                    if key in seen:
                        continue
                    seen.add(key)
                    positions.append((game_index, ply, board.copy()))
                    if len(positions) == a.positions:
                        break
    if len(positions) != a.positions:
        p.error(f'PGN yielded {len(positions)} distinct positions, requested {a.positions}')
    data = {'selection': 'unique positions every fourth ply from ply 12, in file order',
            'wave_nodes': a.wave_nodes, 'outcome_band_cp': 150,
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
            wave_cp = {}
            limit = chess.engine.Limit(nodes=a.wave_nodes) if a.wave_nodes else chess.engine.Limit(time=a.movetime_ms/1000)
            for label, engine in engines.items():
                answer = engine.analyse(board, limit, game=object())
                selected[label] = answer['pv'][0]
                wave_cp[label] = answer['score'].pov(board.turn).score(mate_score=10000)
            evaluated = {best['pv'][0]: best_cp}
            mate = best['score'].pov(board.turn).mate()
            mates = {best['pv'][0]: mate}
            for move in sorted(set(selected.values()) - evaluated.keys(), key=chess.Move.uci):
                value = oracle.analyse(board, chess.engine.Limit(nodes=a.oracle_nodes),
                                       root_moves=[move], game=object())
                mates[move] = value['score'].pov(board.turn).mate()
                evaluated[move] = value['score'].pov(board.turn).score(mate_score=10000)
            # Equal finite node budgets can disagree; retain raw values and use
            # the strongest observed reference rather than negative move loss.
            reference = max(best_cp, *evaluated.values())
            row = {'game': game_index, 'ply': ply, 'fen': board.fen(),
                   'phase': 'endgame' if chess.popcount(board.occupied & ~(board.kings | board.pawns)) <= 4 else ('opening' if ply <= 20 else 'middlegame'),
                   'geometry': geometry(board), 'oracle_mate': mate,
                   'oracle_best': best['pv'][0].uci(), 'oracle_cp': best_cp, 'moves': {}}
            for label, move in selected.items():
                after = board.copy()
                after.push(move)
                allows_mate = any(after.gives_check(reply) and _mates(after, reply)
                                  for reply in after.legal_moves)
                row['moves'][label] = {'move': move.uci(), 'oracle_cp': evaluated[move],
                    'wave_cp': wave_cp[label], 'wave_band': band(wave_cp[label]), 'oracle_band': band(best_cp),
                    'missed_mate_within_n': mate is not None and mate > 0 and not (mates[move] is not None and 0 < mates[move] <= mate),
                    'loss_cp': reference - evaluated[move], 'allows_mate_in_one': allows_mate}
            avoidable = (any(m['allows_mate_in_one'] for m in row['moves'].values())
                         and any(not _allows_mate(board, move) for move in board.legal_moves))
            for move in row['moves'].values():
                move['avoidable_mate_in_one'] = move['allows_mate_in_one'] and avoidable
            data['rows'].append(row)
            if len(data['rows']) % 100 == 0:
                a.output.write_text(json.dumps(data) + '\n')
            print(json.dumps({'position': len(data['rows']), 'moves': row['moves']}), flush=True)
    summarize(data)
    a.output.write_text(json.dumps(data, indent=2) + '\n')
    print(json.dumps(data['summary'], indent=2))


def summarize(data):
    data['summary'] = {}
    for label in data['engines']:
        moves = [r['moves'][label] for r in data['rows']]
        data['summary'][label] = {'positions': len(moves),
            'mean_loss_cp': round(statistics.mean(m['loss_cp'] for m in moves), 2),
            'median_loss_cp': statistics.median(m['loss_cp'] for m in moves),
            'missed_mate_within_n': sum(m['missed_mate_within_n'] for m in moves),
            'loss_at_least_300cp': sum(m['loss_cp'] >= 300 for m in moves),
            'loss_at_least_200cp': sum(m['loss_cp'] >= 200 for m in moves),
            'loss_at_least_100cp': sum(m['loss_cp'] >= 100 for m in moves),
            'allows_mate_in_one': sum(m['allows_mate_in_one'] for m in moves),
            'avoidable_mate_in_one': sum(m['avoidable_mate_in_one'] for m in moves)}
        matrices = {}
        for row in data['rows']:
            m = row['moves'][label]
            groups = ['all', row['phase']]
            for motif in ('absolute_pin', 'skewer_geometry', 'xray'):
                condition = motif if motif in row['geometry'] else 'no_' + motif
                groups.extend([condition, row['phase'] + '/' + condition])
            for group in groups:
                matrix = matrices.setdefault(group, {x: dict.fromkeys(('win', 'equal', 'loss'), 0)
                                                     for x in ('win', 'equal', 'loss')})
                matrix[m['oracle_band']][m['wave_band']] += 1
        data['summary'][label]['oracle_rows_wave_columns'] = matrices


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
