# Chronos TUI repository roadmap

The capability sequence is owned by Chronos's `docs/tui-roadmap.md`. This
repository owns the reusable Ratatui implementation and its host contract.

The standalone binary now lives in the separate `chronos-tui-cli` workspace
package. The library package has no `src/main.rs`; existing interactive hub,
classic, profile, and legacy snapshot behavior is retained. The explicit
`headless` command now observes one or more workstreams through the bounded
V9 workflow observer, emits a versioned `tasks.workflow.observe.v9` JSON report
with authoritative status and separate execution diagnostics, preserves
healthy rows on partial failure, and constructs no TUI state. The real-service
tmux hub and feed journeys pass through the extracted binary.
The peer now publishes a closed interactive/headless presentation capability
manifest. Standalone and Workbench hosts can negotiate it before connecting;
the manifest never enables task writes or scheduling authority.
Standalone headless configuration failures retain the versioned peer envelope,
and service observations distinguish not-sent, outcome-unknown, invalid reply,
semantic refusal, resynchronization, limit, and capacity states.
The peer's new-task form now composes 1–128 unique task IDs and submits the
exact bounded set through Chronos's existing atomic `SubmitGraph` owner command.
The singular path remains one row; Tab adds visible rows and Enter remains the
separate submission action. Embedded qualification proves that N rows use one
mutation request and one post-receipt projection rather than a host-side loop.
Task cancellation now solves the naturally plural case without inventing owner
atomicity: Space builds a bounded marked set (the current task remains the
singular fallback), every child is reauthorized against the latest revision and
uses its own exact V3 invocation receipt, and an unknown outcome stops the run
with all later children retained as not sent. Model qualification includes a
lost-reply cut proving that the third child is never dispatched.
The same marked set now drives task pause and not-before controls. V5 has no
multi-task atomic command or retained invocation lookup, so the peer reobserves
overview and exact eligibility before each child, preserves the other
eligibility field, exposes independent `sA/sR/s?/sN` task badges, and stops on
uncertainty. A lost-second-reply test proves that the third control is not sent.

- Keep `chronos-tui` composable: no CLI parsing, terminal lifecycle, service
  startup, global shortcut policy, or process exit in the component boundary.
- Keep the Chronos standalone CLI/application as a separate composition root
  that can run the TUI or bounded headless commands and snapshots.
- Let Workbench import the component, never the standalone CLI shell, and use
  the same public client whether attaching to or hosting Chronos.
- Qualify standalone interactive/headless and Workbench interactive/headless
  paths for write opt-in, revisions, confirmations, receipts, recovery, and
  outcome-unknown fencing.

Hosted composition may share one Workbench executable and lifecycle, but task
authority and durable state remain owned by Chronos.
