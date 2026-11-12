#!/usr/bin/env python3
"""Small sequential UCI arena: Stockfish Elo ladder or paired version comparison.

Only python-chess is required. No evaluation adjudication, GUI, numpy or scipy.
UCI_Elo anchors are nominal Stockfish settings, not measured human ratings.
"""
import argparse
import contextlib
import hashlib
import json
import math
import platform
import random
import statistics
import time
from collections import defaultdict
from pathlib import Path

import chess
import chess.engine
import chess.pgn

from arena import HOLDOUT


def expected(rating, anchor):
    return 1 / (1 + 10 ** max(-30, min(30, (anchor - rating) / 400)))


def fit(samples, score=None):
    """Invert the logistic expected score; draws contribute half a point."""
    count = sum(n for _, _, n in samples)
    score = sum(s for _, s, _ in samples) if score is None else score
    if score <= 0:
        return -math.inf
    if score >= count:
        return math.inf
    lo, hi = -20000.0, 20000.0
    for _ in range(65):
        mid = (lo + hi) / 2
        if sum(n * expected(mid, r) for r, _, n in samples) < score:
            lo = mid
        else:
            hi = mid
    return (lo + hi) / 2


def finite(value):
    return round(value, 1) if math.isfinite(value) else ('-inf' if value < 0 else '+inf')


def rating_summary(rows, bootstrap=2000):
    pairs = defaultdict(list)
    for row in rows:
        pairs[(row['anchor'], row['opening'])].append(row)
    complete = [group for group in pairs.values()
                if len(group) == 2 and all(r['score'] is not None for r in group)
                and {r['candidate_white'] for r in group} == {False, True}]
    if not complete:
        return {'estimate': None, 'rated_games': 0, 'reason': 'No complete colour pairs'}
    # Reuse of an opening across levels is NOT counted as independent evidence.
    clusters = defaultdict(list)
    for group in complete:
        clusters[group[0]['opening']].append((group[0]['anchor'], sum(r['score'] for r in group), 2))
    units = list(clusters.values())
    samples = [sample for unit in units for sample in unit]
    estimate = fit(samples)
    rng = random.Random(20261112)
    replicates = sorted(fit([s for _ in units for s in rng.choice(units)]) for _ in range(bootstrap))
    # Also report a conservative bounded-score interval. A repeated opening is
    # one independent cluster; unequal cluster sizes use weighted Hoeffding.
    n = sum(s[2] for s in samples)
    score = sum(s[1] for s in samples)
    weight_sq = sum((sum(s[2] for s in unit) / n) ** 2 for unit in units)
    error = math.sqrt(math.log(40) * weight_sq / 2)
    return {
        'estimate': finite(estimate), 'rated_games': n,
        'excluded_games': len(rows) - n, 'opening_clusters': len(units),
        'bootstrap_95': [finite(replicates[int(bootstrap * .025)]), finite(replicates[int(bootstrap * .975)])],
        'conservative_95': [finite(fit(samples, score - error * n)), finite(fit(samples, score + error * n))],
        'method': 'logistic expected-score fit; paired opening-cluster bootstrap; weighted Hoeffding bounds',
        'warning': 'Conditional on nominal opponent ratings and independent opening clusters. Not FIDE/online Elo. '
                   'Few openings, short time controls and excluded games limit inference.',
    }


def summary(rows, bootstrap=2000):
    by_level = {}
    for anchor in sorted({r['anchor'] for r in rows}):
        part = [r for r in rows if r['anchor'] == anchor]
        by_level[str(anchor)] = {
            'wins': sum(r['score'] == 1 for r in part),
            'draws': sum(r['score'] == .5 for r in part),
            'losses': sum(r['score'] == 0 for r in part),
            'unfinished': sum(r['score'] is None for r in part),
        }
    return {'levels': by_level, 'rating': rating_summary(rows, bootstrap)}


def start_engine(stack, path, extra):
    exe = Path(path).resolve(strict=True)
    engine = stack.enter_context(chess.engine.SimpleEngine.popen_uci([str(exe)], timeout=5))
    defaults = {'Threads': 1, 'Hash': 16, 'Move Overhead': 0, 'Opening Variety': 0, 'Random Seed': 42}
    options = {k: v for k, v in defaults.items() if k in engine.options}
    if 'UCI_LimitStrength' in engine.options:
        options['UCI_LimitStrength'] = False
    options.update(extra)
    engine.configure(options)
    return engine, {'id': engine.id, 'path': str(exe),
                    'sha256': hashlib.sha256(exe.read_bytes()).hexdigest(), 'options': options}


def play_game(candidate, opponent, board, white, milliseconds, max_plies, game_id):
    sides = {chess.WHITE: 'candidate' if white else 'opponent',
             chess.BLACK: 'opponent' if white else 'candidate'}
    engines = {'candidate': candidate, 'opponent': opponent}
    wall = defaultdict(list)
    depths = defaultdict(list)
    nodes = defaultdict(int)
    error = None
    for _ in range(max_plies):
        if board.is_game_over(claim_draw=True):
            break
        label = sides[board.turn]
        start = time.perf_counter()
        try:
            result = engines[label].play(board, chess.engine.Limit(time=milliseconds / 1000),
                                         game=game_id, info=chess.engine.INFO_BASIC)
            wall[label].append((time.perf_counter() - start) * 1000)
            if result.move not in board.legal_moves:
                raise ValueError(f'illegal move {result.move}')
            depths[label].append(result.info.get('depth', 0))
            nodes[label] += result.info.get('nodes', 0)
            board.push(result.move)
        except (chess.engine.EngineError, TimeoutError, ValueError) as exc:
            error = f'{label}: {type(exc).__name__}: {exc}'
            break
    outcome = board.outcome(claim_draw=True)
    score = None if outcome is None else (.5 if outcome.winner is None else float(outcome.winner == white))
    return {
        'score': score, 'result': outcome.result() if outcome else '*',
        'termination': outcome.termination.name if outcome else ('ENGINE_ERROR' if error else 'PLY_LIMIT_UNFINISHED'),
        'error': error, 'final_fen': board.fen(),
        'timing_ms': {k: {'sum': round(sum(v), 2), 'mean': round(statistics.mean(v), 2),
                         'max': round(max(v), 2), 'moves': len(v)} for k, v in wall.items()},
        'mean_reported_depth': {k: round(statistics.mean(v), 2) for k, v in depths.items()},
        'nodes': dict(nodes),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', required=True)
    opponent = parser.add_mutually_exclusive_group(required=True)
    opponent.add_argument('--stockfish')
    opponent.add_argument('--reference', help='Version comparison; output rating is an Elo difference')
    parser.add_argument('--elos', default='1320,1520,1720,1920')
    parser.add_argument('--pairs', type=int, default=8, help='Colour pairs per level (maximum 16 distinct openings)')
    parser.add_argument('--opening-offset', type=int, default=0)
    parser.add_argument('--movetime-ms', type=int, default=60)
    parser.add_argument('--max-plies', type=int, default=400)
    parser.add_argument('--engine-options', default='{}')
    parser.add_argument('--opponent-options', default='{}')
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    if not 1 <= args.pairs <= len(HOLDOUT) or args.movetime_ms < 1 or args.max_plies < 1:
        parser.error('pairs must be 1..16; time and max-plies must be positive')
    if args.output.suffix.lower() != '.json':
        parser.error('--output must end in .json; PGN uses a separate .pgn file')
    if args.output.exists() or args.output.with_suffix('.pgn').exists():
        parser.error('output exists; choose a new filename to preserve results')
    levels = [int(x) for x in args.elos.split(',')] if args.stockfish else [0]
    if len(set(levels)) != len(levels):
        parser.error('duplicate Elo levels')
    extra = json.loads(args.opponent_options)
    if args.stockfish and set(extra) & {'UCI_Elo', 'UCI_LimitStrength', 'Skill Level'}:
        parser.error('ladder strength is controlled by --elos')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    data = {
        'format': 1, 'mode': 'nominal_stockfish_elo' if args.stockfish else 'relative_elo',
        'platform': platform.platform(), 'python_chess': chess.__version__,
        'movetime_ms': args.movetime_ms, 'max_plies': args.max_plies,
        'draw_claims': True, 'adjudication': False, 'sequential': True,
        'levels': levels, 'pairs_per_level': args.pairs, 'games': [],
    }
    with contextlib.ExitStack() as stack:
        candidate, data['candidate'] = start_engine(stack, args.engine, json.loads(args.engine_options))
        opponent, data['opponent'] = start_engine(stack, args.stockfish or args.reference, extra)
        if args.stockfish:
            if opponent.id.get('name') != 'Stockfish 19':
                parser.error('expected official Stockfish 19 UCI identity')
            elo_option = opponent.options.get('UCI_Elo')
            if not elo_option or any(not elo_option.min <= level <= elo_option.max for level in levels):
                parser.error(f'levels outside advertised UCI_Elo range: {elo_option}')
            data['stockfish_elo_range'] = [elo_option.min, elo_option.max]
        pgn = stack.enter_context(args.output.with_suffix('.pgn').open('w', encoding='utf-8'))
        # Interleave levels by opening to avoid putting one level only at the
        # beginning or end of a long session with changing machine conditions.
        for pair in range(args.pairs):
            opening = (pair + args.opening_offset) % len(HOLDOUT)
            for level in levels:
                if args.stockfish:
                    opponent.configure({'UCI_LimitStrength': True, 'UCI_Elo': level})
                for white in (True, False):
                    board = chess.Board()
                    for move in HOLDOUT[opening]:
                        board.push_uci(move)
                    initial_fen = board.fen()
                    row = play_game(candidate, opponent, board, white, args.movetime_ms, args.max_plies, object())
                    row.update({'game': len(data['games']) + 1, 'opening': opening,
                                'anchor': level, 'candidate_white': white, 'initial_fen': initial_fen,
                                'opponent_strength': {'UCI_LimitStrength': True, 'UCI_Elo': level} if args.stockfish else {}})
                    data['games'].append(row)
                    game = chess.pgn.Game.from_board(board)
                    game.headers.update({'Event': 'WaveChess quick Elo',
                        'White': 'candidate' if white else 'opponent', 'Black': 'opponent' if white else 'candidate',
                        'Result': row['result'], 'Termination': row['termination'],
                        'TimeControl': '?', 'MoveTimeMs': str(args.movetime_ms), 'NominalOpponentElo': str(level)})
                    pgn.write(str(game) + '\n\n')
                    pgn.flush()
                    data['summary'] = summary(data['games'], bootstrap=200)
                    args.output.write_text(json.dumps(data, indent=2, allow_nan=False) + '\n', encoding='utf-8')
                    print(json.dumps({'game': row['game'], 'anchor': level, 'white': white,
                                      'result': row['result'], 'termination': row['termination']}), flush=True)
                    if row['error']:
                        raise RuntimeError(row['error'])
        data['summary'] = summary(data['games'])
        args.output.write_text(json.dumps(data, indent=2, allow_nan=False) + '\n', encoding='utf-8')
        print(json.dumps(data['summary'], indent=2), flush=True)


if __name__ == '__main__':
    main()
