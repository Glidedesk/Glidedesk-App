# syntax=docker/dockerfile:1.7
#
# glidedesk-builder — the only place Glidedesk is compiled, tested and packaged.
#
# Produces: Windows x64/ARM64 (cargo-xwin + NSIS), macOS arm64/x86_64/universal
# (clang + ld64.lld against the host's macOS SDK, mounted at /opt/macos-sdk at
# run time), Linux host build for tests.
#
# The macOS SDK is NEVER baked into this image (Apple licence: Apple hardware
# only). `make sdk` (run on a Mac) copies it into ~/.cache/glidedesk and the
# Makefile mounts it read-only.

# Pinned by digest (Dependabot updates it); the tag says which image it is.
FROM rust:1-trixie@sha256:a8a5f0a1e5fe7dfe1d352591e4a1c7dd2c08fd70475cae872cf3458ba0df0546

ARG NODE_MAJOR=24
ARG DEBIAN_FRONTEND=noninteractive

# ---- system toolchain ------------------------------------------------------
RUN apt-get update && apt-get install -y --no-install-recommends \
        clang lld llvm \
        nsis \
        xorriso \
        cmake ninja-build pkg-config \
        zlib1g-dev libbz2-dev libssl-dev \
        xz-utils zstd unzip jq ca-certificates curl git \
        python3 \
    && rm -rf /var/lib/apt/lists/*

# ld64.lld is shipped as ld64.lld-<ver>; expose unversioned names used by our linker wrappers.
RUN set -eux; \
    for t in ld64.lld lld-link llvm-lipo llvm-rc llvm-lib llvm-ar llvm-strip clang-cl; do \
      src="$(ls /usr/bin/${t}-* 2>/dev/null | sort -V | tail -n1 || true)"; \
      [ -n "$src" ] && [ ! -e /usr/bin/$t ] && ln -s "$src" /usr/bin/$t || true; \
    done; \
    command -v ld64.lld lld-link llvm-lipo clang-cl

# ---- Rust targets & cargo tools ------------------------------------------
RUN rustup component add rustfmt clippy \
 && rustup target add \
        x86_64-pc-windows-msvc aarch64-pc-windows-msvc \
        aarch64-apple-darwin x86_64-apple-darwin

RUN cargo install --locked cargo-xwin cargo-nextest cargo-deny apple-codesign \
 && rm -rf /usr/local/cargo/registry /usr/local/cargo/git

# ---- libdmg-hfsplus (dmg compression, same tool Bitcoin Core uses) ---------
RUN set -eux; \
    git clone --depth 1 https://github.com/fanquake/libdmg-hfsplus /tmp/dmg; \
    cmake -S /tmp/dmg -B /tmp/dmg/build -G Ninja -DCMAKE_BUILD_TYPE=Release; \
    cmake --build /tmp/dmg/build; \
    install -m 0755 "$(find /tmp/dmg/build -type f -name dmg -perm -u+x | head -n1)" /usr/local/bin/dmg; \
    rm -rf /tmp/dmg

# ---- Node LTS + pnpm (UI build and tests) ----------------------------------
RUN set -eux; \
    arch="$(dpkg --print-architecture)"; case "$arch" in amd64) n=x64;; arm64) n=arm64;; *) exit 1;; esac; \
    base="https://nodejs.org/dist/latest-v${NODE_MAJOR}.x"; \
    curl -fsSLO "$base/SHASUMS256.txt"; \
    file="$(grep -o "node-v[0-9.]*-linux-${n}.tar.xz" SHASUMS256.txt | head -n1)"; \
    curl -fsSLO "$base/$file"; \
    grep " $file\$" SHASUMS256.txt | sha256sum -c -; \
    tar -xJf "$file" -C /usr/local --strip-components=1 --no-same-owner; \
    rm -f "$file" SHASUMS256.txt; \
    npm install -g pnpm@12.6.0; \
    node --version; pnpm --version

# ---- Linux app build, signing and packaging tools (added for v2) ------------
RUN apt-get update && apt-get install -y --no-install-recommends \
        libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev \
        osslsigncode minisign file openssl \
    && rm -rf /var/lib/apt/lists/*
# nfpm: pinned version, checked against its published SHA-256 (same as CI).
ARG NFPM_VERSION=2.47.0
ARG NFPM_SHA256_X86_64=0660ca602b2d2d2ae4781a06c692b3eeb9d437ffea05b831d76e41f4a3188783
ARG NFPM_SHA256_ARM64=1c0f5f2999b9a974bfb04fdb0cc3306096de530ac5dbb25d739cc5f5219c919c
RUN set -eux; \
    case "$(dpkg --print-architecture)" in \
      amd64) a=x86_64; sum="$NFPM_SHA256_X86_64";; arm64) a=arm64; sum="$NFPM_SHA256_ARM64";; *) exit 1;; esac; \
    f="nfpm_${NFPM_VERSION}_Linux_${a}.tar.gz"; \
    curl -fsSLo "/tmp/$f" "https://github.com/goreleaser/nfpm/releases/download/v${NFPM_VERSION}/$f"; \
    echo "$sum  /tmp/$f" | sha256sum -c -; \
    tar -xzf "/tmp/$f" -C /usr/local/bin nfpm; \
    rm -f "/tmp/$f"; \
    nfpm --version

# ---- cross-compile configuration ------------------------------------------
COPY docker/bin/ /usr/local/bin/
RUN chmod 0755 /usr/local/bin/gd-*

ENV MACOS_SDK=/opt/macos-sdk \
    MACOSX_DEPLOYMENT_TARGET=13.0 \
    XWIN_ACCEPT_LICENSE=1 \
    XWIN_CROSS_COMPILER=clang \
    XWIN_CACHE_DIR=/cache/xwin \
    CARGO_HOME=/usr/local/cargo \
    CARGO_TARGET_DIR=/cache/target \
    PNPM_HOME=/cache/pnpm \
    npm_config_store_dir=/cache/pnpm-store \
    CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=gd-macos-cc \
    CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER=gd-macos-cc \
    CC_aarch64_apple_darwin=gd-macos-cc \
    CC_x86_64_apple_darwin=gd-macos-cc \
    CXX_aarch64_apple_darwin=gd-macos-cxx \
    CXX_x86_64_apple_darwin=gd-macos-cxx \
    AR_aarch64_apple_darwin=llvm-ar \
    AR_x86_64_apple_darwin=llvm-ar

WORKDIR /src
