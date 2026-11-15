#!/bin/sh
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd -P)
chronos=$(CDPATH= cd -- "$repo/../chronos" && pwd -P)
scratch=$(mktemp -d /tmp/chronos-tui-headless.XXXXXX)
state="$scratch/state"
daemon_target="$scratch/daemon-target"
tui_target="$scratch/tui-target"
daemon="$daemon_target/debug/chronos-service"
tui="$tui_target/debug/chronos-tui"
log="$scratch/service.log"
pid=

cleanup() {
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    "$daemon" --shutdown "$state" >/dev/null 2>&1 || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -rf "$scratch"
}
trap cleanup EXIT HUP INT TERM

mkdir "$state"
chmod 700 "$state"
cargo build --manifest-path "$chronos/Cargo.toml" --package chronos-standalone \
  --bin chronos-service --locked --offline --target-dir "$daemon_target"
cargo build --manifest-path "$repo/Cargo.toml" --package chronos-tui-cli \
  --bin chronos-tui --locked --offline --target-dir "$tui_target"

"$daemon" "$state" --atomic-state --observer-history >"$log" 2>&1 &
pid=$!
attempt=0
while ! "$daemon" --status "$state" 2>/dev/null | grep -q 'ready=true'; do
  if ! kill -0 "$pid" 2>/dev/null || [ "$attempt" -eq 100 ]; then
    sed -n '1,120p' "$log" >&2
    echo 'FAIL: Chronos daemon did not become ready' >&2
    exit 1
  fi
  attempt=$((attempt + 1))
  sleep 0.05
done

report=$("$tui" headless "$state" --name Primary)
printf '%s' "$report" | python3 -c '
import json, sys
report = json.load(sys.stdin)
assert report["version"] == 1 and report["mode"] == "headless"
assert report["peer"] == "chronos"
assert report["complete"] and report["capability"] == "tasks.observe.v1"
assert len(report["workstreams"]) == 1
row = report["workstreams"][0]
assert row["name"] == "Primary" and row["status"] == "current"
assert row["tasks"] == 0 and not row["truncated"] and row["revision"]
'

profile="$scratch/profile.json"
printf '%s\n' \
  '{"version":1,"workstreams":[{"name":"Primary","directory":"state"},{"name":"Offline","directory":"offline"}]}' \
  >"$profile"
set +e
report=$("$tui" headless --profile "$profile" 2>/dev/null)
status=$?
set -e
if [ "$status" -eq 0 ]; then
  echo 'FAIL: partial Chronos observation reported aggregate success' >&2
  exit 1
fi
printf '%s' "$report" | python3 -c '
import json, sys
report = json.load(sys.stdin)
assert not report["complete"]
assert [row["status"] for row in report["workstreams"]] == ["current", "not_sent"]
'

echo 'PASS: real Chronos service -> standalone multi-workstream headless observation'
