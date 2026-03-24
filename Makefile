test:
	cargo test --all

build:
	cargo build --release
	$(eval DEST := $(shell which longbridge 2>/dev/null || echo /usr/local/bin/longbridge))
	@echo "Installing to $(DEST)"
	sudo cp target/release/longbridge $(DEST)

test-commands:
	bun run scripts/test-commands.ts
