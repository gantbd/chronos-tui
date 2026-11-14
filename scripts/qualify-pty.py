"""Exercise Chronos TUI keyboard input in a private synthetic PTY."""

import argparse
import codecs
from contextlib import contextmanager
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import selectors
import socket
import struct
import subprocess
import tempfile
import termios
import time


class Screen:
    """Track the cursor-addressed output used by the Ratatui PTY fixture."""

    csi = re.compile(r"\x1b\[([0-?]*)([ -/]*)([@-~])")

    def __init__(self, rows, columns):
        self.rows = rows
        self.columns = columns
        self.cells = [[" "] * columns for _ in range(rows)]
        self.row = self.column = 0
        self.pending = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")()

    def feed(self, data):
        stream = self.pending + self.decoder.decode(data)
        offset = 0
        while offset < len(stream):
            char = stream[offset]
            if char == "\x1b":
                if offset + 1 == len(stream):
                    break
                if stream[offset + 1] != "[":
                    offset += 2
                    continue
                match = self.csi.match(stream, offset)
                if match is None:
                    break
                params, _, command = match.groups()
                if command in ("H", "f"):
                    position = (params.split(";") + ["", ""])[:2]
                    self.row, self.column = (max(0, int(value or "1") - 1)
                                             for value in position)
                elif command == "J" and params in ("2", "3"):
                    self.cells = [[" "] * self.columns for _ in range(self.rows)]
                offset = match.end()
                continue
            if char == "\r":
                self.column = 0
            elif char == "\n":
                self.row += 1
            elif char.isprintable() and self.row < self.rows and self.column < self.columns:
                self.cells[self.row][self.column] = char
                self.column += 1
            offset += 1
        self.pending = stream[offset:]

    def text(self):
        return "\n".join("".join(row) for row in self.cells)


@contextmanager
def service(binary, state, *flags):
    child = subprocess.Popen([binary, state, "--atomic-state", *flags],
                             stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        with selectors.DefaultSelector() as readiness:
            readiness.register(child.stderr, selectors.EVENT_READ)
            if not readiness.select(30):
                raise RuntimeError("synthetic service readiness deadline")
            if child.stderr.readline().strip() != b"Chronos local service ready":
                raise RuntimeError("synthetic service failed to start")
        yield
    finally:
        if child.poll() is None:
            child.terminate()
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
        child.stderr.close()
    if child.returncode != 0:
        raise RuntimeError(f"synthetic service exited with {child.returncode}")


def readback(state, version, command):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(3)
        connection.connect(str(state / "chronos.sock"))
        connection.sendall(json.dumps({"version": version, "command": command}).encode() + b"\n")
        with connection.makefile("rb") as response:
            frame = response.readline(262145)
    if len(frame) > 262144 or not frame.endswith(b"\n"):
        raise RuntimeError("invalid readback framing")
    reply = json.loads(frame)
    if reply["version"] != version:
        raise RuntimeError("unexpected readback version")
    return reply["outcome"]


def run_terminal(binary, state):
    master, slave = pty.openpty()
    child = None
    timings = {}
    try:
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 132, 0, 0))
        initial_mode = termios.tcgetattr(slave)
        child = subprocess.Popen(
            [binary, state, "--classic", "--allow-writes", "--scheduling"],
            stdin=slave, stdout=slave, stderr=slave,
            env={**os.environ, "TERM": "xterm-256color"},
        )
        with selectors.DefaultSelector() as terminal:
            terminal.register(master, selectors.EVENT_READ)
            screen = Screen(32, 132)

            def step(label, keys, marker, timeout=5):
                text = marker.decode("ascii")
                if keys and text in screen.text():
                    raise RuntimeError(f"{label} marker was present before input")
                started = time.monotonic()
                if keys:
                    os.write(master, keys)
                seen = 0
                while True:
                    if not terminal.select(max(0, started + timeout - time.monotonic())):
                        raise RuntimeError(f"{label} missing {text!r}: {screen.text()[-1200:]!r}")
                    data = os.read(master, 8192)
                    seen += len(data)
                    screen.feed(data)
                    if text in screen.text():
                        timings[label] = round((time.monotonic() - started) * 1000, 3)
                        return
                    if seen > 131072 or time.monotonic() >= started + timeout:
                        raise RuntimeError(f"{label} exceeded terminal observation bound")

            step("initial_view", b"", b"Observed 0 tasks", timeout=30)
            step("task_form", b"n", b"New task ID")
            step("submit", b"pty-proof\r", b"Applied.")
            step("observe_disabled", b"s", b"Observed scheduling.")
            step("propose_enable", b"e", b"Enable scheduling controls?")
            step("confirm_enable", b"y", b"Scheduling control applied.")
            step("observe_enabled", b"s", b"Observed scheduling.")
            step("capacity_form", b"b", b"Concurrency capacity:")
            step("propose_capacity", b"3\r", b"Set concurrency capacity to 3?")
            step("confirm_capacity", b"y", b"Scheduling control applied.")
            step("observe_capacity", b"s", b"slots=Some(3)")
            step("not_before_form", b"d", b"Not-before: Unix seconds or clear")
            step("propose_not_before", b"100000\r", b"Set eligibility for pty-proof")
            step("confirm_not_before", b"y", b"Scheduling control applied.")
            step("observe_not_before", b"s", b"not-before=100000.000000000")
            step("propose_task_pause", b"t", b"Set eligibility for pty-proof")
            step("confirm_task_pause", b"y", b"Scheduling control applied.")
            step("observe_task_pause", b"s", b"Selected pause=true not-before=100000.000000000")
            step("propose_pause", b"p", b"Set global pause to true?")
            step("confirm_pause", b"y", b"Scheduling control applied.")
            step("refresh_task", b"r", b"Observed 1 tasks")
            step("propose_cancel", b"c", b"Cancel pty-proof?")
            step("confirm_cancel", b"y", b"cancelled")
            os.write(master, b"q")
            if child.wait(timeout=5) != 0:
                raise RuntimeError("terminal exited unsuccessfully")
        if termios.tcgetattr(slave) != initial_mode:
            raise RuntimeError("terminal mode was not restored")
    finally:
        if child is not None and child.poll() is None:
            child.kill()
            child.wait()
        os.close(master)
        os.close(slave)
    return timings


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tui", type=Path)
    parser.add_argument("service", type=Path)
    args = parser.parse_args()
    tui = args.tui.resolve(strict=True)
    daemon = args.service.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="ch-pty-", dir=Path("/tmp").resolve()) as path:
        state = Path(path) / "state"
        with service(daemon, state, "--allow-writes"):
            timings = run_terminal(tui, state)
        with service(daemon, state):
            task = readback(state, 1, {"operation": "snapshot", "task": "pty-proof"})
            scheduling = readback(state, 5, {
                "operation": "overview",
                "now": {"before_epoch": False, "seconds": "0", "nanos": 0},
            })
            if task["value"]["snapshot"]["phase"] != "cancelled":
                raise RuntimeError("reopened task was not cancelled")
            if not scheduling["controls"]["paused"]:
                raise RuntimeError("reopened scheduling was not paused")
            if scheduling["concurrency"]["capacity"] != 3:
                raise RuntimeError("reopened capacity did not match")
            eligibility = readback(state, 5, {
                "operation": "eligibility",
                "task": task["value"]["reference"],
            })["eligibility"]
            if eligibility != {
                "paused": True,
                "not_before": {"before_epoch": False, "seconds": "100000", "nanos": 0},
            }:
                raise RuntimeError("reopened task eligibility did not match")
    print(json.dumps({"input_to_terminal_output_ms": timings, "durable_reopen": True}, indent=2))


if __name__ == "__main__":
    main()
