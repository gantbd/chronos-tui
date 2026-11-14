"""Qualify Chronos TUI from exact local Git objects in an isolated directory."""

import io
import json
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile


ROOT = Path(__file__).resolve().parents[1]
INPUTS = ("chronos", "moira", "daimon")


def git(checkout, *args):
    return subprocess.check_output(["git", "-C", str(checkout), *args])


def archive(checkout, revision, destination):
    destination.mkdir()
    data = git(checkout, "archive", "--format=tar", revision)
    with tarfile.open(fileobj=io.BytesIO(data)) as source:
        source.extractall(destination, filter="data")


def main():
    # Read the source contract from the same revision that will be tested.
    revision = git(ROOT, "rev-parse", "HEAD").decode().strip()
    contract = json.loads(git(ROOT, "show", f"{revision}:release/source.json"))
    inputs = contract["inputs"]
    missing = [
        name for name in INPUTS
        if not re.fullmatch(r"[0-9a-f]{40}", inputs[name]["revision"] or "")
    ]
    if missing:
        raise SystemExit(f"Missing exact committed sibling revisions: {', '.join(missing)}")

    with tempfile.TemporaryDirectory(prefix="chronos-source-", dir="/tmp") as tmp:
        stage = Path(tmp)
        archive(ROOT, revision, stage / "chronos-tui")
        for name in INPUTS:
            checkout = ROOT / inputs[name]["checkout"]
            archive(checkout, inputs[name]["revision"], stage / name)
        subprocess.run(["make", "check"], cwd=stage / "chronos-tui", check=True)
    print(f"Qualified local committed source closure for chronos-tui {revision}")


if __name__ == "__main__":
    main()
