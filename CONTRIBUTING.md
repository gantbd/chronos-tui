# Contributing

Keep the reusable component presentation-only. Scheduling and cancellation
authority must remain in Chronos-owned clients, with uncertain mutations fenced
until an explicit refresh.

Run `make check` before submitting a change. Keep the lockfile current and run
`cargo audit --deny warnings` when dependencies change.
