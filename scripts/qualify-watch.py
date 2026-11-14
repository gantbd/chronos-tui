"""Qualify opt-in Chronos feed recovery and terminal response in tmux."""

import argparse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import runpy
import shlex
import subprocess
import tempfile
import time


def qualify(tui, daemon):
    helpers = runpy.run_path(str(Path(__file__).with_name("qualify-pty.py")))
    service, request = helpers["service"], helpers["readback"]
    with tempfile.TemporaryDirectory(prefix="chronos-watch-", dir="/private/tmp") as directory:
        root = Path(directory)
        main, peer = root / "main", root / "peer"

        def tmux(*args, check=True):
            return subprocess.run(
                ["tmux", "-S", str(root / "tmux.sock"), "-f", "/dev/null", *args],
                capture_output=True, text=True, check=check,
            )

        def keys(*values):
            tmux("send-keys", "-t", "chronos", *values)

        def expect(*markers):
            deadline = time.monotonic() + 30
            while True:
                screen = tmux("capture-pane", "-p", "-t", "chronos").stdout
                if all(marker in screen for marker in markers):
                    return
                if time.monotonic() >= deadline:
                    raise AssertionError(f"missing {markers!r}\n{screen}")
                time.sleep(0.02)

        def expected(revision):
            return {"kind": "finite", "value": str(revision)}

        def mutate(state, version, command):
            reply = request(state, version, command)
            assert reply["outcome"] == "applied", reply

        def submit(state, revision, task):
            mutate(state, 1, {
                "operation": "submit_graph", "expected": expected(revision),
                "tasks": [{
                    "id": task, "priority": 0, "dependencies": [], "capabilities": [],
                    "max_attempts": 1, "retry_safety": "fence_unknown",
                }],
            })

        def snapshot(state, task):
            return request(state, 1, {"operation": "snapshot", "task": task})["value"]

        flags = ("--allow-writes", "--observer-history")
        timings = []
        with service(daemon, peer, *flags):
            try:
                with service(daemon, main, *flags):
                    tmux("new-session", "-d", "-x", "142", "-y", "33", "-s", "chronos",
                         shlex.join([str(tui), str(main), "--name", "Main", "--workstream",
                                     "Peer", str(peer), "--allow-writes", "--watch"]))
                    expect("Main  ·  revision 0", "feed observed")
                    submit(main, 0, "reusable")
                    expect("Main  ·  revision 1", "› reusable")
                    keys("c")
                    expect("Cancel reusable?")
                    original = snapshot(main, "reusable")["reference"]
                    mutate(main, 1, {"operation": "cancel", "expected": expected(1), "task": original})
                    mutate(main, 2, {"operation": "retire", "expected": expected(2), "task": original})
                    submit(main, 3, "reusable")
                    expect("feed refresh pending", "Cancel reusable?")
                    keys("y")
                    expect("Main  ·  revision 4", "feed observed", "› reusable")
                    replacement = snapshot(main, "reusable")
                    assert replacement["reference"] != original
                    assert replacement["snapshot"]["phase"] == "ready"

                    def change_peer():
                        for i in range(10):
                            submit(peer, i, f"peer-{i}")
                            time.sleep(0.12)

                    with ThreadPoolExecutor(max_workers=1) as executor:
                        work = executor.submit(change_peer)
                        for _ in range(5):
                            started = time.monotonic()
                            keys("w")
                            expect("Switch workstream")
                            timings.append(round((time.monotonic() - started) * 1000, 2))
                            keys("Escape")
                            expect("Board", "Inspector")
                            time.sleep(0.15)
                        work.result()

                    keys("3", "c")
                    expect("Cancel reusable?")
                    keys("x")
                    expect("Ready for authorized claim")
                expect("feed offline or unavailable", "Cancel reusable?")
                with service(daemon, main, *flags):
                    expect("feed resync pending", "Cancel reusable?")
                    keys("y")
                    expect("Discard the old intent and refresh.")
                    keys("Escape")
                    expect("Main  ·  revision 4", "feed observed", "x explain readiness")
                    screen = tmux("capture-pane", "-p", "-t", "chronos").stdout
                    assert "Ready for authorized claim" not in screen
                    keys("x")
                    expect("Ready for authorized claim")
                    assert snapshot(main, "reusable")["snapshot"]["phase"] == "ready"
                    keys("w")
                    tmux("send-keys", "-t", "chronos", "-l", "peer")
                    keys("Enter")
                    expect("Peer  ·  revision 10", "feed observed")
                    keys("q")
                print(f"tmux key-to-captured-output milliseconds under mixed writes: {timings}")
                print("Feed qualification passed: concurrent changes, stale confirmation, generation gap, and restart recovery")
            finally:
                tmux("kill-server", check=False)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tui", type=lambda value: Path(value).resolve(strict=True))
    parser.add_argument("service", type=lambda value: Path(value).resolve(strict=True))
    options = parser.parse_args()
    qualify(options.tui, options.service)
