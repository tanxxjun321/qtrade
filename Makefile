all:
	cargo build
	./target/debug/qtrade start

release:
	cargo build --release
	./target/release/qtrade start
