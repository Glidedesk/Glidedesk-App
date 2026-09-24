# Glidedesk — every build and code test runs inside the glidedesk-builder
# container (PLAN §8.4). Output lands in output-build/.

RUN := docker/run.sh
IMAGE := glidedesk-builder:latest
CROSS_TARGETS := aarch64-apple-darwin x86_64-pc-windows-msvc

.PHONY: help signing-keys docker-image sdk shell fmt lint test test-rust test-ui check-cross build-win build-mac build-linux release clean

help:
	@echo "make docker-image  build the builder image"
	@echo "make sdk           copy the macOS SDK from this Mac (needed for macOS builds)"
	@echo "make test          fmt + clippy + all tests + cross-target checks (in Docker)"
	@echo "make build-win     Windows x64 installers          -> output-build/"
	@echo "make build-mac     macOS Apple Silicon .dmg        -> output-build/"
	@echo "make build-linux   Linux .deb/.rpm/.tar.gz (this arch) -> output-build/"
	@echo "make release       everything                      -> output-build/"
	@echo "make shell         interactive shell in the builder"

docker-image:
	docker build -f docker/Dockerfile -t $(IMAGE) .

sdk:
	docker/copy-sdk.sh

signing-keys:
	$(RUN) scripts/generate-signing-keys.sh

shell:
	$(RUN) bash

fmt:
	$(RUN) cargo fmt --all

lint:
	$(RUN) sh -c 'cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings'

test-rust:
	$(RUN) cargo nextest run --workspace --locked --no-fail-fast

test-ui:
	$(RUN) sh -c 'cd app/ui && pnpm install --frozen-lockfile && pnpm test'

check-cross:
	$(RUN) cargo xtask check-cross $(CROSS_TARGETS)

test: lint test-rust check-cross test-ui

build-win:
	$(RUN) cargo xtask package windows

build-mac:
	$(RUN) cargo xtask package macos

build-linux:
	$(RUN) cargo xtask package linux

release:
	$(RUN) cargo xtask release

clean:
	$(RUN) cargo clean
	rm -rf output-build/*
