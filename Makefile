test:
	cargo test --all

build:
	cargo build
	$(eval DEST := $(shell which longport 2>/dev/null || echo /usr/local/bin/longport))
	@echo "Installing to $(DEST)"
	sudo cp target/debug/longport $(DEST)

test-commands:
	bun run scripts/test-commands.ts
