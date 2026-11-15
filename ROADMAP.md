# Chronos TUI repository roadmap

The capability sequence is owned by Chronos's `docs/tui-roadmap.md`. This
repository owns the reusable Ratatui implementation and its host contract.

The standalone binary now lives in the separate `chronos-tui-cli` workspace
package. The library package has no `src/main.rs`; existing interactive hub,
classic, profile, and legacy snapshot behavior is retained. The explicit
`headless` command now observes one or more workstreams through the bounded
public observer, emits a versioned `tasks.observe.v1` JSON report, preserves
healthy rows on partial failure, and constructs no TUI state. The real-service
tmux hub and feed journeys pass through the extracted binary.

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
