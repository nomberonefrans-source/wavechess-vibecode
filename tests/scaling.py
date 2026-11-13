#!/usr/bin/env python3
"""Sequential CPU/hash benchmark. Fixed work and equal time are separate tests.

Repeated-position warm-cache timings are an upper bound on reuse, not Elo.
No concurrent matches should run on the benchmark machine.
"""
import argparse
import hashlib
import json
import os
import platform
import statistics
import time
from pathlib import Path

import chess
import chess.engine
from compare import CASES


def run(args):
    output = Path(args.output)
    if output.exists():
        raise SystemExit('output already exists')
    exe = Path(args.engine).resolve(strict=True)
    quota = Path('/sys/fs/cgroup/cpu.max')
    data = {'engine_sha256': hashlib.sha256(exe.read_bytes()).hexdigest(),
            'platform': platform.platform(), 'cpu_count': os.cpu_count(),
            'affinity': sorted(os.sched_getaffinity(0)) if hasattr(os, 'sched_getaffinity') else None,
            'cpu_max': quota.read_text().strip() if quota.exists() else None,
            'depth': args.depth, 'movetime_ms': args.movetime_ms, 'rows': []}
    positions = [(n, f) for n, f, _, _ in CASES if n in ('start', 'kiwipete', 'middlegame', 'ep_endgame')]
    for threads in map(int, args.threads.split(',')):
        for memory in map(int, args.hashes.split(',')):
            with chess.engine.SimpleEngine.popen_uci(str(exe), timeout=120) as engine:
                engine.configure({'Threads': threads, 'Hash': memory, 'Opening Variety': 0,
                                  'Move Overhead': 0, 'UCI_AnalyseMode': True})
                for name, fen in positions:
                    board = chess.Board(fen)
                    game = object()
                    for mode in ('cold', 'warm', 'equal_time'):
                        if mode == 'equal_time':
                            game = object()
                        limit = (chess.engine.Limit(time=args.movetime_ms / 1000)
                                 if mode == 'equal_time' else chess.engine.Limit(depth=args.depth))
                        started = time.perf_counter()
                        info = engine.analyse(board, limit, game=game)
                        row = {'threads': threads, 'hash_mb': memory, 'case': name, 'mode': mode,
                               'wall_ms': round(1000 * (time.perf_counter() - started), 3),
                               'engine_ms': round(1000 * info.get('time', 0)),
                               'nodes': info.get('nodes'), 'depth': info.get('depth'),
                               'score': str(info['score'].pov(board.turn)),
                               'bestmove': info['pv'][0].uci(), 'diagnostics': info.get('string')}
                        data['rows'].append(row)
                        print(json.dumps(row), flush=True)
                        output.write_text(json.dumps(data, indent=2) + '\n')
    data['summary'] = []
    for threads in map(int, args.threads.split(',')):
        for memory in map(int, args.hashes.split(',')):
            for mode in ('cold', 'warm', 'equal_time'):
                rows = [r for r in data['rows'] if (r['threads'], r['hash_mb'], r['mode']) == (threads, memory, mode)]
                data['summary'].append({'threads': threads, 'hash_mb': memory, 'mode': mode,
                    'total_engine_ms': sum(r['engine_ms'] for r in rows),
                    'total_wall_ms': round(sum(r['wall_ms'] for r in rows), 2),
                    'total_nodes': sum(r['nodes'] for r in rows),
                    'median_depth': statistics.median(r['depth'] for r in rows)})
    # Completed fixed-depth results must be independent of physical resources.
    for name, _ in positions:
        completed = [r for r in data['rows'] if r['case'] == name and r['mode'] != 'equal_time']
        assert len({(r['score'], r['bestmove'], r['depth'], r['nodes']) for r in completed}) == 1, name
    data['fixed_work_identical'] = True
    output.write_text(json.dumps(data, indent=2) + '\n')


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--engine', required=True)
    p.add_argument('--threads', default='1,2,4')
    p.add_argument('--hashes', default='0,1,16,128')
    p.add_argument('--depth', type=int, default=12)
    p.add_argument('--movetime-ms', type=int, default=250)
    p.add_argument('--output', required=True)
    run(p.parse_args())
