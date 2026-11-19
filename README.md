# chronos-tui

Independent, unpublished Ratatui terminal presentation for Chronos. The local
source build uses sibling `chronos`, `moira`, and `daimon` checkouts. The
[source record](release/source.json) tracks the current qualification limits.
The [repository roadmap](ROADMAP.md) records the TUI/CLI/Workbench boundary.

```sh
make check
cargo run -p chronos-tui-cli --bin chronos-tui --locked --offline -- /private/tmp/chronos-local
# Optional writes require both service and client enablement:
cargo run -p chronos-tui-cli --bin chronos-tui --locked --offline -- /private/tmp/chronos-local --allow-writes
cargo run --example composite --locked --offline -- /private/tmp/chronos-local
```

The host separately starts `chronos-service` from the Chronos checkout. Private
same-user Unix sockets are the first transport, not remote authentication.
Each client has its own page, selection, bounded marked set and exact
cancellation confirmation.
Unknown mutation outcomes fence further writes until explicit refresh; never
retry automatically. No execution agents or shared authoritative task state
live in this presentation.

The new-task form is an atomic batch composer, not a one-record demo. Type one
task ID for the familiar singular case, or press Tab to add rows before Enter.
It shows the selected row count, current UTF-8 byte count, and complete bounded
draft; one submission sends one owner-side `SubmitGraph` containing 1–128
unique IDs. Each ID is limited to 256 bytes. The TUI never loops over
individual create calls or turns partial child success into an atomic claim.

Embed `Component::new(client, writes)`, call nonblocking `poll()`, route
closed `Input` values through `update(input, focused)`, and render with
`draw_in(frame, rect)`. `Lifecycle::CloseRequested` is output; the host decides
whether to remove a pane or exit. Key mapping belongs here, never to the engine.
The host owns terminal setup/restoration, event routing, geometry and focus.
The composite example uses Tab to switch focus and q to close only the active
pane. Rendering tests prove no writes outside the supplied region; real PTY
qualification exercised submit/confirm, sibling survival and clean exit.

Injected clients may be local transport or `chronos_sdk_embedded::native_client::EmbeddedClient`.
Each instance admits one action and one view handoff; background service waits
never run in its UI update path. Content-free client request/refusal/error and
residence signals remain separate from engine/service/persistence signals.
Batch qualification asserts that N tasks produce one mutation request, while
Chronos's existing engine and service instrumentation records the content-free
`SubmitGraph` operation and task count.

## Source qualification

Use `make check` and `make qualify-tmux` for working-tree qualification.
`make qualify-source` extracts the committed TUI and pinned sibling
revisions into a temporary directory and runs the locked offline quality
gate. The sibling revisions are on local `qualification/chronos-tui-source`
branches. The source record identifies the tested TUI revision. Remote
reachability remains unqualified.

## Coordination hub

The normal launcher opens the hub. Each supplied directory is an existing
Chronos service with its own task state and permissions. Use `Project / Workstream`
names to group adjacent workstreams under a project in the sidebar.

```sh
cargo run -p chronos-tui-cli --bin chronos-tui --locked --offline -- /private/tmp/atlas-planning \
  --name 'Atlas / Planning' \
  --workstream 'Boreal / Delivery' /private/tmp/boreal-delivery \
  --allow-writes --scheduling
```

Press Tab, then Up or Down to switch workstreams. Enter returns focus to the
task view. Each workstream keeps its own page, selection, and pending
confirmation. The header names the active service and shows its observed
revision. Press 1–7 for Board, Tasks, Dependencies, Schedule, Execution, Overview, or Plan.
Overview shows counts from each workstream’s observed task page and flags
pending confirmations. Press Enter on a workstream to open its board.
The inspector follows task selection. Narrow terminals prioritize the active
view. An embedding host may bind one Chronos-owned product-alignment endpoint
to an exact workstream; **8 Alignment** then presents its current proposal,
stakeholder cohort, evidence/dissent/gate counts, and recommended next tasks.
It is refreshable with **r** and has no apply, acceptance, claim,
reprioritization, or sign-off authority. Use `--classic` for the original
single-service presentation.

Board lanes group the six authoritative statuses: Backlog, Open, Blocked,
In Progress, Ready for Review, and Done. Left and Right move to the next
populated lane. Execution phase remains separate diagnostic evidence.
Dependencies shows observed prerequisite counts and readiness. Press
**x** on a selected task to request the authorized V2 explanation, including
remaining prerequisites and downstream reference count. The inspector displays
it only for the matching task incarnation and revision. Schedule shows the
selected not-before timestamp and service capacity. Execution shows observed attempt numbers. The current local
V6 protocol supplies visible dependency identities. Task date ranges and
agent roster or assignment data remain prerequisites for Gantt and agent
management.

Run the hub qualification with an isolated tmux server and synthetic services:

```sh
python3 scripts/qualify-tmux.py target/debug/chronos-tui ../chronos/target/debug/chronos-service
```

### Saved workspaces and task search

Append `--save-profile /path/to/team.json` to the directory-based launch
command to save its workstream names and endpoints and exit. Reopen it with:

```sh
cargo run -p chronos-tui-cli --bin chronos-tui --locked --offline -- --profile /path/to/team.json --scheduling
```

The versioned JSON file stores connection configuration only. Pass
`--allow-writes` explicitly when opening a writable session. Saving publishes
a new private file and refuses an existing destination. Relative directories
in hand-edited profiles resolve from the profile’s directory.

Press **w** and type part of a workstream name to switch directly to its board.
Press **/** for the cross-workstream task queue. Search task IDs case-insensitively
and optionally add `status:open`, `execution:ready`, or `in:atlas`. Each result names the originating
workstream, even when task IDs match. Up and Down select, Enter opens the source
board and Escape closes the search. The search uses observed pages and reports
workstreams with additional pages or unverified data. Refresh and paginate in
a workstream to change the observed page. Search does not dispatch task mutations.

### Feed freshness

Start the service with `--observer-history` and add `--watch` when opening
the hub or a workspace profile. The hub pulls one bounded V3 batch per
workstream per second, off the terminal thread. Changed revisions and feed
gaps trigger a fresh V1 first page. Selection follows the same task incarnation
if it is still present. The header and Overview report each feed's status.

Drafts and pending confirmations defer background refresh. A feed outage or
generation change fences new mutations until the view is resynchronized.
If a confirmation was open, discard it with Escape before recovery. After an
unknown mutation result or a scheduling control, press **r** to explicitly
refresh. The watcher never retries a mutation. Omit `--watch` for manual
refresh against a service without observer history.

```sh
python3 scripts/qualify-watch.py target/debug/chronos-tui ../chronos/target/debug/chronos-service
```

### Dependency graph

From **3 Dependencies**, select a task and press **g**. Left shows its visible
prerequisites, Right shows visible dependents, and Enter follows the selected
edge. Escape returns to the task view. The graph is read-only. It returns up
to 32 visible neighbors in each direction and flags additional visible edges.
An empty view means no visible edges. Denied, stale and unavailable reads
have distinct messages. The graph requires a V6-capable Chronos service.
The local daemon uses the same trusted operator read policy for each endpoint
task. Hosts with differentiated visibility can apply their own per-edge policy.

```sh
python3 scripts/qualify-tmux.py target/debug/chronos-tui ../chronos/target/debug/chronos-service --dependencies-v6
```

### Eligibility agenda

With `--scheduling`, press **4** for Schedule and **a** to load the selected
window of up to eight observed task rows. The agenda sorts exact not-before
timestamps and shows paused and unscheduled tasks. Use Up or Down to reach
another window, then press **a**. Press **4** again to return to the scheduling
summary. If the service revision changes during a read, refresh the task page
with **r** before loading the agenda again. These timestamps represent claim
eligibility. Planned date ranges and due dates require a planning source.

### Planning dates

Launch the hub with `--planning PATH` and press **7** to open the Gantt.
The supplied JSON file is a read-only planning source. Workstream names in it
are display labels, so they can cover work outside the connected services.
Use Up and Down to inspect a row’s exact start, end, and due dates.

```json
{
  "version": 1,
  "items": [
    {
      "workstream": "Atlas / Planning",
      "label": "Prepare review",
      "start": "2026-11-12",
      "end": "2026-11-15",
      "due": "2026-11-17"
    }
  ]
}
```

Dates are inclusive Gregorian civil dates in `YYYY-MM-DD` format. Start,
end, and due are optional. A bar requires both start and end. Planning labels
and dates are independent of Chronos task identity and claim eligibility.

## Scheduling controls

Add `--scheduling` to opt into the V5 client against an explicitly selected
atomic local service. Embeddings use `Component::new_with_scheduling(client,
writes)` and provide the V1 mutation plus V9 workflow observation ports. The same option works
with `--snapshot` without entering terminal mode. Reads need no write flag.

For a bounded machine-readable observation across one or more workstreams, use
the explicit headless command:

```sh
cargo run -p chronos-tui-cli --bin chronos-tui --locked --offline -- \
  headless /path/to/primary --name Primary \
  --workstream Secondary /path/to/secondary
```

It emits a versioned JSON report for the `tasks.workflow.observe.v9`
capability. Every bounded task entry includes its authoritative status and
separate execution phase; blocker evidence is present only for Blocked work.
The report retains each healthy workstream when another is unavailable and
exits unsuccessfully unless every observation is current. `--snapshot`
remains the legacy single-workstream human-readable form.

Run `sh tests/live/headless.sh` to qualify the explicit headless command against
a real Chronos service, including a healthy workstream retained beside an
offline peer and a nonzero aggregate exit.

`chronos-tui capabilities` prints the peer-owned interactive/headless
presentation manifest without opening a service. It describes composition
support and does not enable writes, scheduling controls, or task authority.

Press `s` to inspect scheduling, `e` to propose enablement, `p` for global pause,
`t` for marked/current task pause, `b` for concurrency capacity, and `d` for
marked/current not-before. Space uses the same bounded task workset as
cancellation; an empty set falls back to the focused task. Task eligibility is
not an atomic owner operation: every child is reobserved against the latest
owner revision, preserves its unrelated eligibility field, and retains an
applied/refused/unknown/not-sent disposition. Unknown results stop the run and
leave later children unsent. Each proposal requires `y`; Escape discards.
Pausing affects new claims and does not interrupt execution. Read-only, stale
and busy views cannot submit control forms, and there is no automatic replay or
executor-stop attestation.

The not-before form accepts supported nonnegative whole Unix seconds or `clear`.
Existing fractional or pre-epoch timestamps are displayed exactly and preserved
when toggling pause. Replacing/clearing a time is explicit; no timezone/calendar
or recurrence interpretation is selected. Forms remain byte bounded.

Headless tests exercise actual input, model/service behavior and bounded rendering.
Run the synthetic keyboard journey against built TUI and service binaries:

```sh
python3 scripts/qualify-pty.py target/debug/chronos-tui ../chronos/target/debug/chronos-service
```

The probe reconstructs rendered terminal cells, submits and cancels a synthetic
task, enables scheduling, changes capacity and not-before, and exercises task
pause and global pause. It checks terminal mode restoration and reopens the
service to verify the exact persisted controls. One expanded run against C2
observed 32–59 ms from input to rendered output for scheduling confirmations
and 32 ms for cancellation. These are fixture observations on one host.

### Inspect an execution attempt

Configure the authenticated peer sockets for each workstream you want to inspect.

```sh
chronos-tui --profile workspace.json \
  --moira Atlas /path/to/moira.sock atlas-service-scope \
  --daimon Atlas /path/to/daimon.sock
```

Press **5** for Execution, select an attempt, then press **x**. Moira's host must
enable the attempt lookup policy and establish the service-scope and task
mapping. The TUI sends the exact task incarnation and attempt. If Moira
discloses a recorded execution ID, the optional Daimon endpoint authenticates
and authorizes a separate status read. The inspector shows the retained Moira
receipt and current Daimon status independently. These flags apply to the
current hub session.

### Receipted task cancellation

Start Chronos with `--atomic-state --allow-writes`, then open the hub with
`--allow-writes --cancel-receipts`. Press **Space** to mark tasks on the current
bounded page, then **c** and **y**; with no marks, the focused task is the
one-item default. This is deliberately not described as an atomic batch.
Chronos reobserves the owner revision before every child, submits a distinct V3
invocation, and retains an applied/refused/unknown/not-sent disposition for each
task. An uncertain reply triggers one read-only lookup of that exact invocation.
If its receipt remains unavailable, the run stops, later children remain visibly
not sent, and explicit refresh is required. This option applies to the current
hub session.

### Host-bound text handoff

A composition host can call `Hub::bind_handoff` to expose export and review in
Tasks. Press **h**, then **Enter**, to confirm export for the selected task.
Press **v** to request review. The host receives the exact task incarnation and
observed revision. It resolves the live source association, authorizes access
and publication, and owns the review proposal. After an unknown publication,
inspect the destination with the host before starting a new export session.
The standalone binary still needs a live Vellum host composition.


### Daimon presence

With a Daimon peer configured, open **5 Execution** and press **l** to read
the bounded roster of Daimon-supervised agents. Press **l** again to refresh,
use the arrow keys to scroll, and press **Esc** to return. The view shows the
snapshot age and the presence status Daimon reported at read time.
