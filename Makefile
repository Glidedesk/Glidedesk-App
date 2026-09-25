# Glidedesk — every build and code test runs inside the glidedesk-builder
# container (PLAN §8.4). Output lands in output-build/.

RUN := docker/run.sh
IMAGE := glidedesk-builder:latest
CROSS_TARGETS := aarch64-apple-darwin x86_64-pc-windows-msvc

.PHONY: help signing-keys docker-image sdk shell fmt lint test test-rust test-ui check-cross build-win build-mac build-linux release clean lab lab-stop os-smoke

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

# Real end-to-end lab: server + two Linux clients (one with two monitors) on virtual X screens and
# dummy networks, settings UI at http://localhost:5173/?agent=server (client-a, client-b).
lab:
	$(RUN) cargo build -p glidedesk-app
	docker build -q -t glidedesk-lab -f docker/lab/Dockerfile docker/lab
	docker rm -f gd-lab >/dev/null 2>&1 || true
	docker run -d --name gd-lab --cap-add NET_ADMIN -p 127.0.0.1:5173:5173 -v "$(CURDIR):/src" \
	  -v gd-cargo-registry:/usr/local/cargo/registry -v gd-cargo-git:/usr/local/cargo/git -v gd-cache:/cache \
	  -w /src glidedesk-lab:latest sh -c 'cd app/ui && pnpm install --frozen-lockfile >/dev/null 2>&1; /src/docker/lab/lab.sh && sleep infinity'

lab-stop:
	docker rm -f gd-lab

# Real server + client agents over loopback (connect, client restarts, self-test) on a
# virtual X screen — the same script CI runs on Linux, Windows and macOS on every push.
os-smoke:
	$(RUN) sh -c 'cd app/ui && pnpm install --frozen-lockfile >/dev/null && pnpm build >/dev/null && cd /src && cargo build -p glidedesk-app'
	docker build -q -t glidedesk-lab -f docker/lab/Dockerfile docker/lab
	docker run --rm -v "$(CURDIR):/src" -v gd-cache:/cache -w /src glidedesk-lab:latest \
	  sh -c 'Xvfb :99 -screen 0 1920x1080x24 -nolisten tcp >/dev/null 2>&1 & sleep 1; DISPLAY=:99 bash scripts/os-smoke.sh /cache/target/debug/glidedesk'
