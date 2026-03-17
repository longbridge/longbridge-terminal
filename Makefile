test:
	cargo test --all

build:
	cargo build --release
	sudo cp target/release/longbridge /usr/local/bin/longbridge

test-commands:
	bun run scripts/test-commands.ts
