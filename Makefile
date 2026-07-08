test:
	RUST_MIN_STACK=8388608 cargo test --all

build:
	cargo build
	$(eval DEST := $(shell which longbridge 2>/dev/null || echo /usr/local/bin/longbridge))
	@echo "Installing to $(DEST)"
	sudo cp target/debug/longbridge $(DEST)

test-commands:
	bun run scripts/test-commands.ts
