all: build

build:
	cargo build --release
	mkdir -p bin
	cp -f target/release/obol bin
	cp -f target/release/lockbud bin

build-offline:
	cargo build --release --offline
	mkdir -p bin
	cp -f target/release/obol bin
	cp -f target/release/lockbud bin

build-dev:
	cargo build
	mkdir -p bin
	cp -f target/debug/obol bin
	cp -f target/debug/lockbud bin

test-lockbud:
	python3 scripts/test_lockbud.py

test-lockbud-all:
	python3 scripts/test_lockbud.py --all

.PHONY: all build build-offline build-dev test-lockbud test-lockbud-all
