.PHONY: test clippy fmt check

test:
	cargo test --all-targets

clippy:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt

check: fmt test clippy
