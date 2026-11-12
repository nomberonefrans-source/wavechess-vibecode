#!/usr/bin/env python3
"""UCI integration regressions; run after `cargo build --release`.

Usage: python3 tests/uci_protocol.py [path/to/wavechess]
No Python packages are required.
"""
import pathlib
import queue
import subprocess
import sys
import threading
import time
import unittest

ENGINE = pathlib.Path(sys.argv.pop(1) if len(sys.argv) > 1 else "target/release/wavechess").resolve()


class Engine:
    def __init__(self):
        self.proc = subprocess.Popen(
            [str(ENGINE), "uci"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, bufsize=1,
        )
        self.lines = queue.Queue()
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        self.send("uci")
        self.handshake = self.until(lambda line: line == "uciok")
        self.send("setoption name Threads value 1\nsetoption name Hash value 1\nisready")
        self.until(lambda line: line == "readyok")

    def _read(self):
        for line in self.proc.stdout:
            self.lines.put(line.rstrip())
        self.lines.put(None)

    def send(self, command):
        self.proc.stdin.write(command + "\n")
        self.proc.stdin.flush()

    def until(self, predicate, timeout=8):
        deadline = time.monotonic() + timeout
        seen = []
        while True:
            left = deadline - time.monotonic()
            if left <= 0:
                raise AssertionError(f"UCI timeout; received {seen[-10:]}")
            try:
                line = self.lines.get(timeout=left)
            except queue.Empty:
                raise AssertionError(f"UCI timeout; received {seen[-10:]}") from None
            if line is None:
                raise AssertionError(f"Engine exited unexpectedly: {self.proc.stderr.read()}")
            seen.append(line)
            if predicate(line):
                return seen

    def close(self):
        if self.proc.poll() is None:
            try:
                if not self.proc.stdin.closed:
                    self.send("quit")
                    self.proc.stdin.close()
                self.proc.wait(timeout=5)
            except (BrokenPipeError, subprocess.TimeoutExpired):
                self.proc.kill()
                self.proc.wait(timeout=5)
        self.reader.join(timeout=1)
        for pipe in (self.proc.stdin, self.proc.stdout, self.proc.stderr):
            pipe.close()


class Protocol(unittest.TestCase):
    def setUp(self):
        self.engine = Engine()
        self.addCleanup(self.engine.close)

    def test_removed_recursive_solver_options(self):
        text = "\n".join(self.engine.handshake)
        self.assertIn("WaveChess 3.1", text)
        self.assertNotIn("Mate Defense", text)
        self.assertNotIn("Mate Attack", text)

    def test_infinite_waits_and_isready_during_analysis(self):
        self.engine.send("position startpos\ngo infinite")
        first = self.engine.until(lambda line: line.startswith("info depth "))
        self.assertFalse(any(line.startswith("bestmove") for line in first))
        self.engine.send("isready")
        ready = self.engine.until(lambda line: line == "readyok", timeout=2)
        self.assertFalse(any(line.startswith("bestmove") for line in ready))
        self.engine.send("stop")
        self.engine.until(lambda line: line.startswith("bestmove "))

    def test_queued_stops_are_not_lost(self):
        # Keeping stdin open is essential: EOF must not rescue a lost stop.
        count = 12
        self.engine.send("position startpos\n" + "go infinite\nstop\n" * count)
        for _ in range(count):
            self.engine.until(lambda line: line.startswith("bestmove "), timeout=5)
        self.engine.send("isready")
        self.engine.until(lambda line: line == "readyok", timeout=2)

    def test_eof_cancels_active_and_queued_infinite_analysis(self):
        self.engine.send("position startpos\ngo infinite\ngo infinite\ngo infinite")
        self.engine.proc.stdin.close()
        self.assertEqual(self.engine.proc.wait(timeout=5), 0)

    def test_invalid_weights_and_extreme_clock_do_not_panic(self):
        self.engine.send(
            "setoption name W:Control value NaN\n"
            "setoption name W:ReplyK value 9999999\n"
            "setoption name W:ReplyTau value -1\n"
            "position startpos\n"
            "go wtime -9223372036854775808 btime 1 winc 9223372036854775807"
        )
        lines = self.engine.until(lambda line: line.startswith("bestmove "), timeout=5)
        self.assertNotEqual(lines[-1], "bestmove 0000")
        self.assertFalse(any("NaN" in line or "inf" in line.split() for line in lines))

    def test_fen_only_updates_keep_real_repetition_history(self):
        fens = [
            "rnbqkbnr/pppppppp/8/8/8/5N2/PPPPPPPP/RNBQKB1R b KQkq - 1 1",
            "rnbqkb1r/pppppppp/5n2/8/8/5N2/PPPPPPPP/RNBQKB1R w KQkq - 2 2",
            "rnbqkb1r/pppppppp/5n2/8/8/8/PPPPPPPP/RNBQKBNR b KQkq - 3 2",
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 4 3",
        ]
        self.engine.send("position startpos")
        for fen in fens:
            self.engine.send("position fen " + fen)
        self.engine.send("memory")
        info = self.engine.until(lambda line: line.startswith("info string memory"))[-1]
        self.assertIn("moves 4 positions 5", info)
        self.engine.send("setoption name Clear Hash\nmemory")
        info = self.engine.until(lambda line: line.startswith("info string memory"))[-1]
        self.assertIn("moves 4 positions 5", info)
        self.engine.send("ucinewgame\nmemory")
        info = self.engine.until(lambda line: line.startswith("info string memory"))[-1]
        self.assertIn("moves 0 positions 1", info)

    def test_seeded_variety_and_final_pv_match_bestmove(self):
        self.engine.send("setoption name Random Seed value 42")
        seen = set()
        for _ in range(12):
            self.engine.send("ucinewgame\nposition startpos\ngo depth 1")
            lines = self.engine.until(lambda line: line.startswith("bestmove "))
            best = lines[-1].split()[1]
            pv = [line for line in lines if " multipv 1 " in line and " pv " in line][-1]
            self.assertEqual(pv.split(" pv ")[1].split()[0], best)
            self.assertFalse(any("multipv 2" in line for line in lines))
            seen.add(best)
        self.assertGreaterEqual(len(seen), 2)

    def test_analysis_is_deterministic_and_all_weights_are_advertised(self):
        text = "\n".join(self.engine.handshake)
        self.assertIn("option name W:ReplyK", text)
        self.assertIn("option name W:Spectral type string default 16", text)
        self.engine.send("setoption name UCI_AnalyseMode value true")
        choices = []
        for _ in range(3):
            self.engine.send("ucinewgame\nposition startpos\ngo depth 2")
            choices.append(self.engine.until(lambda line: line.startswith("bestmove "))[-1].split()[1])
        self.assertEqual(len(set(choices)), 1)


if __name__ == "__main__":
    unittest.main()
