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
        self.assertIn("WaveChess 2.2", text)
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


if __name__ == "__main__":
    unittest.main()
