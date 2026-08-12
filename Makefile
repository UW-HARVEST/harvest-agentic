.PHONY: test
test:
	RUSTFLAGS="-D warnings" cargo build
	RUSTFLAGS="-D warnings" cargo test
	RUSTFLAGS="-D warnings" cargo clippy
	cargo fmt --check
	cd nightly && \
		MIRIFLAGS="-Zmiri-disable-isolation" \
		RUSTFLAGS="-D warnings" cargo miri test --manifest-path=../Cargo.toml
