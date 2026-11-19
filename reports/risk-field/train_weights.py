"""Fit scalar move preferences from the labelled wave-prior training archive.

Pass the extracted training directory containing labels0.jsonl, labels1.jsonl
and its compiled export executable. Writes the Rust coefficients to stdout.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

os.environ["OPENBLAS_NUM_THREADS"] = "1"
import numpy as np
from scipy.optimize import minimize
from scipy.special import softmax


def pressure_terms(raw):
    # The predecessor exporter writes one row of 43 public features per move.
    # Only inexpensive terms on the two affected squares are needed here.
    pieces = raw[:, :6]
    mass = pieces @ np.array([1, 3.2, 3.3, 5, 9, 4])
    captured = raw[:, 6]
    at_from, defend_from, at_to, defend_to = raw[:, 18:22].T
    free = at_to == 0
    shared_terms = np.column_stack([
        np.ones(len(raw)), raw[:, 6:18], at_from, defend_from, at_to, defend_to,
        raw[:, 24:30], mass * at_from / (1 + defend_from),
        mass * at_to / (1 + defend_to), captured > 0, captured * free,
        np.minimum(captured - mass, 0) * ~free,
        mass * ((at_from > 0) & (defend_from == 0)), (captured > 0) * free,
        pieces[:, 0] * raw[:, 10] ** 2 / 8,
        (pieces[:, 1] + pieces[:, 2]) * raw[:, 13], pieces[:, 5] * raw[:, 12],
        pieces[:, 4] * raw[:, 17], pieces[:, 3] * raw[:, 10], raw[:, 9] * free,
    ])
    deviations = pieces[:, :, None] * shared_terms[:, None, :]
    return np.column_stack([shared_terms, deviations.reshape(len(raw), -1)])


def training_positions(directory):
    rows = []
    for path in sorted(directory.glob("labels?.jsonl")):
        with path.open() as source:
            rows.extend(json.loads(line) for line in source)
    rows.sort(key=lambda row: row[1])
    for start in range(0, len(rows), 100):
        block = rows[start:start + 100]
        result = subprocess.run(
            [str(directory.resolve() / "export")], check=True, capture_output=True,
            text=True, input="".join(f"{fen}\n" for _, fen, _ in block),
        )
        for (opening, _, labels), line in zip(block, result.stdout.splitlines(), strict=True):
            fields = np.asarray(line.split()).reshape(-1, 44)
            moves = fields[:, 0].tolist()
            scores = np.array([v for _, v in labels])
            ordered = [0, -0.35, -0.7][:len(labels)]
            soft_labels = softmax(np.clip(scores - scores.max(), -500, 0) / 65 + ordered)
            target = np.zeros(len(moves))
            for (move, _), probability in zip(labels, soft_labels):
                target[moves.index(move)] = probability
            group = int(hashlib.sha256(opening.encode()).hexdigest()[:8], 16) % 5
            if group != 0:
                yield pressure_terms(fields[:, 1:].astype(float)), target


def fit(positions):
    features, targets = zip(*positions)
    sizes = np.array([len(target) for target in targets])
    offsets = np.r_[0, np.cumsum(sizes)[:-1]]
    x = np.concatenate(features)
    y = np.concatenate(targets)
    scale = x.std(0).clip(0.1)
    x /= scale

    def objective(weights):
        logits = x @ weights
        logits -= np.repeat(np.maximum.reduceat(logits, offsets), sizes)
        exp = np.exp(logits)
        sums = np.add.reduceat(exp, offsets)
        loss = (np.log(sums).sum() - logits @ y) / len(sizes)
        gradient = x.T @ (exp / np.repeat(sums, sizes) - y) / len(sizes)
        return loss + 0.0005 * (weights @ weights), gradient + 0.001 * weights

    result = minimize(objective, np.zeros(x.shape[1]), method="L-BFGS-B", jac=True,
                      options={"maxiter": 250})
    if not result.success:
        raise RuntimeError(result.message)
    weights = result.x / scale
    return weights[:36] + weights[36:].reshape(6, 36)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("training_directory", type=Path)
    weights = fit(list(training_positions(parser.parse_args().training_directory)))
    for row in weights:
        print("[" + ", ".join(f"{w:.8f}" for w in row) + "],")
