"""Compare UCI engines on EPD best moves. Requires python-chess.

python tests/quick_tactics.py --engine old=./old --engine new=./new
With no EPD paths, download the pinned WAC and ECM suites (483 positions).
Use --depth 1 for a quick evaluation check, --depth 3 for trajectory decisions.
"""
import argparse
import hashlib
import json
import time
from pathlib import Path
from urllib.request import urlopen

import chess
import chess.engine


def suite_texts(paths):
    if paths:
        for path in paths:
            yield path.name, path.read_text()
        return
    revision = 'af92d2ee9533483db8b12809306235c4f17a5637'
    digests = {
        'wacnew.epd': '9bfb7b42cf5bbc7f3d60ff767ca3be460e0ae4d9110ef9a71ddf0bd7071d4d46',
        'ecmgcp.epd': 'b3a8ec4bfc9c2ab49ae9f0ecb8bba4b6a4da8ad1ab1a4534756d71b8c78791de',
    }
    for name, expected in digests.items():
        url = f'https://raw.githubusercontent.com/jdart1/arasan-chess/{revision}/tests/{name}'
        with urlopen(url, timeout=15) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != expected:
            raise ValueError(f'checksum mismatch for {name}')
        yield name, data.decode()


def read_suite(paths):
    cases = []
    for name, text in suite_texts(paths):
        for line_no, line in enumerate(text.splitlines(), 1):
            if not line.strip() or line.lstrip().startswith('#'):
                continue
            board, ops = chess.Board.from_epd(line)
            if not board.is_valid() or not ops.get('bm'):
                raise ValueError(f'{name}:{line_no} requires a valid position and bm')
            cases.append((board, set(ops['bm']), f'{name}:{line_no}'))
    if not cases:
        raise ValueError('empty suite')
    return cases


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('epd', nargs='*', type=Path)
    p.add_argument('--engine', action='append', required=True, metavar='LABEL=PATH')
    budget = p.add_mutually_exclusive_group()
    budget.add_argument('--depth', type=int)
    budget.add_argument('--nodes', type=int)
    p.add_argument('--details', type=Path, help='save per-position answers as JSON')
    args = p.parse_args()
    if args.depth is None and args.nodes is None:
        args.depth = 1
    if any(x is not None and x < 1 for x in (args.depth, args.nodes)):
        p.error('search limit must be positive')
    limit = chess.engine.Limit(depth=args.depth, nodes=args.nodes)
    cases = read_suite(args.epd)
    rows = [{'case': case, 'fen': b.fen(), 'bm': sorted(m.uci() for m in bm), 'answers': {}}
            for b, bm, case in cases]
    baseline = None
    for spec in args.engine:
        name, exe = spec.split('=', 1)
        if name in rows[0]['answers']:
            p.error(f'duplicate label {name}')
        hits = []
        with chess.engine.SimpleEngine.popen_uci(str(Path(exe).resolve())) as engine:
            options = {'Threads': 1, 'Hash': 0, 'Opening Variety': 0}
            engine.configure({k: v for k, v in options.items() if k in engine.options and
                              (engine.options[k].min is None or v >= engine.options[k].min)})
            start = time.monotonic()
            for row, (board, bm, _) in zip(rows, cases):
                info = engine.analyse(board, limit, game=object())
                move = info['pv'][0]
                if move not in board.legal_moves:
                    raise ValueError(f'{name}: illegal move in {row["case"]}')
                hits.append(move in bm)
                row['answers'][name] = move.uci()
            seconds = time.monotonic() - start
        if baseline is None:
            baseline = hits
        print(json.dumps(dict(engine=name, depth=args.depth, nodes=args.nodes, correct=sum(hits), total=len(hits),
                              gained=sum(b and not a for a, b in zip(baseline, hits)),
                              lost=sum(a and not b for a, b in zip(baseline, hits)),
                              seconds=round(seconds, 3))), flush=True)
    if args.details:
        args.details.write_text(json.dumps(rows, indent=2) + '\n')


if __name__ == '__main__':
    main()
