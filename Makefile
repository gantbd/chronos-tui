.PHONY: check fmt lint test doc qualify-tmux qualify-source
check: fmt lint test doc
fmt:
	cargo fmt -p chronos-tui -p chronos-tui-cli -- --check
lint:
	cargo clippy --workspace --all-targets --all-features --locked --offline -- -D warnings
test:
	cargo test --workspace --all-targets --all-features --locked --offline
doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked --offline

# Uses synthetic local services and an isolated tmux server.
qualify-tmux:
	cargo build -p chronos-tui-cli --bin chronos-tui --locked --offline
	cargo build --manifest-path ../chronos/Cargo.toml -p chronos-standalone --bin chronos-service --locked --offline
	python3 scripts/qualify-tmux.py target/debug/chronos-tui ../chronos/target/debug/chronos-service --dependencies-v6 --cancel-receipts
	python3 scripts/qualify-watch.py target/debug/chronos-tui ../chronos/target/debug/chronos-service

qualify-source:
	python3 scripts/qualify-source.py
