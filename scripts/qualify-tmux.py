"""Drive the Chronos hub in an isolated tmux server against private services."""

import argparse
import json
from pathlib import Path
import runpy
import shlex
import subprocess
import tempfile
import time


def run_setup(command, *, stage, check=False):
    try:
        return subprocess.run(
            command, check=check, capture_output=True, text=True, timeout=10,
        )
    except subprocess.TimeoutExpired as error:
        raise RuntimeError(f"{stage} did not exit within 10 seconds") from error


def qualify(tui, daemon, dependencies_v6=False, cancel_receipts=False):
    helpers = runpy.run_path(str(Path(__file__).with_name("qualify-pty.py")))
    service = helpers["service"]
    readback = helpers["readback"]
    with tempfile.TemporaryDirectory(prefix="chronos-hub-", dir="/private/tmp") as directory:
        root = Path(directory)
        first, second = root / "first", root / "second"
        with service(daemon, first, "--allow-writes"), service(daemon, second, "--allow-writes"):
            def tmux(*args, check=True):
                return subprocess.run(
                    ["tmux", "-S", str(root / "tmux.sock"), "-f", "/dev/null", *args],
                    check=check, capture_output=True, text=True,
                )

            profile = root / "coordination.json"
            setup = [
                str(tui), str(first), "--name", "Atlas / Planning",
                "--workstream", "Boreal / Delivery", str(second),
                "--workstream", "Cygnus / Offline", str(root / "offline"),
                "--save-profile", str(profile),
            ]
            run_setup(setup, stage="Save workspace profile", check=True)
            saved = profile.read_bytes()
            assert run_setup(setup, stage="Refuse existing profile").returncode != 0
            assert profile.read_bytes() == saved
            planning = root / "planning.json"
            planning.write_text(json.dumps({"version": 1, "items": [
                {"label": "Build window", "workstream": "Atlas",
                 "start": "2026-11-12", "end": "2026-11-15", "due": "2026-11-17"},
                {"label": "Deadline only", "workstream": "Boreal", "due": "2026-11-19"},
            ]}))
            invalid = root / "invalid-plan.json"
            invalid.write_text(json.dumps({"version": 1, "items": [
                {"label": "Invalid date", "workstream": "Atlas",
                 "start": "2026-02-29", "end": "2026-03-01"},
            ]}))
            refused = run_setup(
                [str(tui), "--profile", str(profile), "--planning", str(invalid)],
                stage="Refuse invalid planning record",
            )
            assert refused.returncode != 0 and "valid YYYY-MM-DD" in refused.stderr
            command = shlex.join([
                str(tui), "--profile", str(profile), "--allow-writes", "--scheduling",
                "--planning", str(planning),
                *(["--cancel-receipts"] if cancel_receipts else []),
            ])
            try:
                tmux("new-session", "-d", "-x", "142", "-y", "33", "-s", "chronos", command)

                def expect(*markers):
                    deadline = time.monotonic() + 30
                    while True:
                        screen = tmux("capture-pane", "-p", "-t", "chronos").stdout
                        if all(marker in screen for marker in markers):
                            return screen
                        if time.monotonic() >= deadline:
                            raise AssertionError(f"missing {markers!r}\n{screen}")
                        time.sleep(0.025)

                def keys(*values):
                    tmux("send-keys", "-t", "chronos", *values)

                def text(value):
                    tmux("send-keys", "-t", "chronos", "-l", value)

                expect("Workstreams", "Inspector", "Blocked 0", "Atlas / Planning", "Observed 0 tasks")
                keys("n")
                expect("New task ID")
                text("atlas-plan")
                keys("Enter")
                expect("Ready 1", "atlas-plan", "Applied.")
                keys("c")
                expect("Cancel atlas-plan?")
                keys("Tab", "Down", "Enter")
                expect("Boreal / Delivery  ·", "Ready 0")
                keys("n")
                expect("New task ID")
                text("boreal-ship")
                keys("Enter")
                board = expect("Ready 1", "boreal-ship", "Applied.")
                assert "atlas-plan" not in board
                for key, marker in (
                    ("2", "Phase          Task"), ("3", "Dependency readiness"),
                    ("4", "Selected task not-before"), ("5", "Execution attempts"),
                ):
                    keys(key)
                    expect(marker)
                keys("6")
                expect("Workstream overview", "Atlas / Planning", "Boreal / Delivery", "confirmation pending")
                keys("Up", "Enter")
                expect("Atlas / Planning  ·", "Cancel atlas-plan?")
                keys("Tab", "Down", "Enter", "4", "s")
                expect("Enabled false", "Observed scheduling.")
                keys("e")
                expect("Enable scheduling controls?")
                keys("y")
                expect("Scheduling control applied.")
                keys("s")
                expect("Enabled true", "Selected task not-before")
                keys("Tab", "Up", "Enter", "1")
                expect("Atlas / Planning  ·", "Cancel atlas-plan?")
                keys("y")
                expect("Closed 1", "cancelled")
                atlas = readback(first, 1, {"operation": "snapshot", "task": "atlas-plan"})
                boreal = readback(second, 1, {"operation": "snapshot", "task": "boreal-ship"})
                assert atlas["value"]["snapshot"]["phase"] == "cancelled", atlas
                if cancel_receipts:
                    next_invocation = readback(first, 3, {"operation": "next_invocation"})
                    assert next_invocation == {"outcome": "next_invocation", "serial": "2"}, next_invocation
                assert boreal["value"]["snapshot"]["phase"] == "ready", boreal
                assert readback(first, 1, {"operation": "snapshot", "task": "boreal-ship"})["value"] is None
                graph = readback(first, 1, {
                    "operation": "submit_graph", "expected": {"kind": "finite", "value": "2"},
                    "tasks": [
                        {"id": task, "priority": 0, "dependencies": dependencies,
                         "capabilities": [], "max_attempts": 1, "retry_safety": "fence_unknown"}
                        for task, dependencies in [("compile", []), ("review", ["compile"])]
                    ],
                })
                assert graph["outcome"] == "applied", graph
                keys("r")
                expect("Blocked 1", "review")
                keys("Left")
                expect("› compile", "Phase        ready")
                keys("Left")
                expect("› review", "Phase        blocked")
                keys("Right")
                expect("› compile", "Phase        ready")
                keys("3", "x")
                expect("Prerequisites", "1               blocked", "review", "Ready for authorized claim", "Referencing tasks 1")
                keys("Down")
                expect("› 1", "Phase        blocked")
                keys("x")
                expect("Awaiting 1 prerequisite(s)", "Referencing tasks 0")
                tmux("resize-window", "-t", "chronos", "-x", "70", "-y", "24")
                keys("Tab")
                expect("Workstreams", "Boreal", "Delivery")
                keys("Down", "Enter", "2")
                expect("boreal-ship", "Inspector")
                for state, revision, size in [(first, "3", 65), (second, "2", 1)]:
                    result = readback(state, 1, {
                        "operation": "submit_graph", "expected": {"kind": "finite", "value": revision},
                        "tasks": [
                            {"id": task, "priority": 0, "dependencies": [],
                             "capabilities": [], "max_attempts": 1, "retry_safety": "fence_unknown"}
                            for task in ["a-shared", *(f"queue-{n:02d}" for n in range(size - 1))]
                        ],
                    })
                    assert result["outcome"] == "applied", result
                tmux("resize-window", "-t", "chronos", "-x", "142", "-y", "33")
                keys("r")
                expect("Boreal / Delivery  ·  revision 3")
                keys("w")
                expect("Switch workstream", "Cygnus / Offline [unverified]")
                text("atlas")
                expect("Search: atlas", "1 workstreams")
                keys("Enter", "3", "x")
                expect("different observation.")
                keys("r")
                expect("Atlas / Planning  ·  revision 4")
                keys("/")
                expect("Cross-workstream tasks", "1 with more pages", "1 unverified")
                text("a-shared phase:ready")
                expect("2 matches", "a-shared · ready · Atlas / Planning", "a-shared · ready · Boreal / Delivery")
                keys("Down", "Enter")
                expect("Boreal / Delivery  ·", "› a-shared")
                keys("/")
                text("a-shared in:atlas")
                expect("1 matches", "a-shared · ready · Atlas / Planning")
                keys("Enter")
                expect("Atlas / Planning  ·", "› a-shared")
                for revision, task, before_epoch, seconds, nanos, paused in [
                    (3, "a-shared", False, "100000", 125000000, False),
                    (4, "boreal-ship", True, "3", 250000000, True),
                ]:
                    reference = readback(second, 1, {"operation": "snapshot", "task": task})["value"]["reference"]
                    applied = readback(second, 5, {
                        "operation": "set_eligibility", "task": reference,
                        "expected": {"kind": "finite", "value": str(revision)},
                        "eligibility": {"paused": paused, "not_before": {
                            "before_epoch": before_epoch, "seconds": seconds, "nanos": nanos,
                        }},
                    })
                    assert applied == {"outcome": "control", "result": {"result": "applied"}}, applied
                keys("w")
                text("boreal")
                keys("Enter", "r")
                expect("Boreal / Delivery  ·  revision 5")
                keys("4", "a")
                agenda = expect("Eligibility agenda", "-3.250000000  [paused] boreal-ship", "100000.125000000  a-shared")
                assert agenda.index("-3.250000000") < agenda.index("100000.125000000")
                if dependencies_v6:
                    result = readback(second, 1, {
                        "operation": "submit_graph", "expected": {"kind": "finite", "value": "5"},
                        "tasks": [
                            {"id": task, "priority": 0, "dependencies": deps,
                             "capabilities": [], "max_attempts": 1, "retry_safety": "fence_unknown"}
                            for task, deps in [
                                ("diamond-root", []), ("diamond-left", ["diamond-root"]),
                                ("diamond-right", ["diamond-root"]),
                                ("diamond-tip", ["diamond-left", "diamond-right"]),
                            ]
                        ],
                    })
                    assert result["outcome"] == "applied", result
                    keys("r")
                    expect("Boreal / Delivery  ·  revision 6")
                    keys("/")
                    text("diamond-tip")
                    expect("1 matches", "diamond-tip · blocked")
                    keys("Enter", "3")
                    expect("Phase        blocked", "diamond-tip")
                    keys("g")
                    expect("Prerequisites of diamond-tip", "diamond-left", "diamond-right")
                    keys("Enter")
                    expect("Prerequisites of diamond-left", "diamond-root")
                    keys("Right")
                    expect("Dependents of diamond-left", "diamond-tip")
                    keys("Enter")
                    expect("Dependents of diamond-tip", "No visible edges")
                    task = readback(second, 1, {"operation": "snapshot", "task": "a-shared"})["value"]["reference"]
                    changed = readback(second, 1, {
                        "operation": "cancel", "expected": {"kind": "finite", "value": "6"}, "task": task,
                    })
                    assert changed["outcome"] == "applied", changed
                    keys("Left")
                    expect("Graph is stale")
                    keys("Escape")
                    print("V6 graph qualified: diamond navigation, empty visible edges, and stale refusal")
                keys("7")
                screen = expect("Gantt", "2026-11-12 to 2026-11-15", "Deadline only", "No planned range")
                assert any("Build window" in line and "━" in line for line in screen.splitlines())
                assert all("━" not in line for line in screen.splitlines() if "Deadline only" in line)
                keys("Down")
                expect("Due 2026-11-19")
                keys("1")
                expect("Board", "Inspector")
                keys("q")
                deadline = time.monotonic() + 5
                while tmux("has-session", "-t", "chronos", check=False).returncode == 0:
                    if time.monotonic() >= deadline:
                        raise AssertionError("hub did not exit on q")
                    time.sleep(0.025)
                print(board)
                print("tmux hub qualification passed: saved profile, duplicate task search, offline service, partial pages, seven views, and guarded cancellation")
            finally:
                tmux("kill-server", check=False)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tui", type=lambda value: Path(value).resolve(strict=True))
    parser.add_argument("service", type=lambda value: Path(value).resolve(strict=True))
    parser.add_argument("--dependencies-v6", action="store_true")
    parser.add_argument("--cancel-receipts", action="store_true")
    arguments = parser.parse_args()
    qualify(arguments.tui, arguments.service, arguments.dependencies_v6, arguments.cancel_receipts)
