# Stage 1: Chef - Rust toolchain + build tools + cargo-chef
# This layer rarely changes; all derived stages benefit from its cache.
FROM rust:1.91-slim AS chef

ARG CARGO_BUILD_JOBS=1

RUN apt-get update -y && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libclang-dev \
    clang \
    curl \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install cargo-chef --locked && \
    cargo install svm-rs --locked && \
    svm install 0.8.24

WORKDIR /kailua

# Stage 2: Planner - resolve the full dependency graph into a recipe
# Only manifest files are copied so source-only changes never re-run the planner
# or invalidate the cook layer downstream.
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
# --parents preserves directory structure; globs auto-discover all workspace members.
# New crates under bin/*, build/*, crates/* are picked up automatically.
COPY --parents bin/*/Cargo.toml build/*/Cargo.toml crates/*/Cargo.toml ./
# cargo metadata requires a stub src file per member to infer targets.
# Crates under bin/ get src/main.rs; everything else gets src/lib.rs.
RUN find . -mindepth 2 -name Cargo.toml | while read f; do \
      dir=$(dirname "$f"); mkdir -p "$dir/src"; \
      case $dir in bin/*) printf 'fn main(){}' > "$dir/src/main.rs" ;; \
                   *)     touch "$dir/src/lib.rs" ;; \
      esac; done
RUN cargo chef prepare --recipe-path recipe.json

# Stage 3: Builder - pre-build all deps, then compile the workspace
FROM chef AS builder

ARG CARGO_BUILD_JOBS=1
# Optional feature flag: "celestia" or "eigen" or empty for base build
ARG BUILD_FEATURES=""

COPY --from=planner /kailua/recipe.json recipe.json
# Make the toolchain version available before cook so rustup selects the right one.
# If rust-toolchain.toml changes, the cook layer is correctly invalidated.
COPY rust-toolchain.toml .
# Copy workspace Cargo.toml so profile.* changes (lto, codegen-units, etc.)
# also invalidate the cook layer, not just dependency graph changes.
COPY Cargo.toml .

# Pre-build all external dependencies (cached independently of source changes).
# Only re-runs when Cargo.toml / Cargo.lock change.
# NOTE: No --mount=type=cache on /kailua/target here — the compiled deps must be
# baked into the layer so that registry-based layer caching (cache-from/cache-to)
# can export and restore them across CI runs.
RUN set -e; \
    FEATURES="disable-dev-mode,prove"; \
    if [ -n "$BUILD_FEATURES" ]; then FEATURES="$FEATURES,$BUILD_FEATURES"; fi; \
    cargo chef cook --jobs ${CARGO_BUILD_JOBS} --release -p kailua-cli --features $FEATURES --recipe-path recipe.json

# Copy real sources — only workspace crates recompile from here.
COPY . .

RUN set -e; \
    FEATURES="disable-dev-mode,prove"; \
    if [ -n "$BUILD_FEATURES" ]; then FEATURES="$FEATURES,$BUILD_FEATURES"; fi; \
    cargo build --jobs ${CARGO_BUILD_JOBS} --release -p kailua-cli --features $FEATURES --locked \
    && mkdir -p out \
    && mv target/release/kailua-cli out/ \
    && strip out/kailua-cli

# Stage 4: Runtime - minimal image, no Rust toolchain needed
# ubuntu:24.04 ships glibc 2.39, required by risc0 precompiled native libs.
# debian:bookworm-slim only has glibc 2.36 and will fail at runtime.
FROM ubuntu:24.04 AS kailua

RUN apt-get update -y && apt-get install -y --no-install-recommends \
    jq \
    ca-certificates \
    curl \
    unzip \
    gnupg \
    && rm -rf /var/lib/apt/lists/*

# Install AWS CLI v2 (bundles its own Python, no system Python required)
RUN curl -fsSL "https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip" -o /tmp/awscliv2.zip \
    && unzip -q /tmp/awscliv2.zip -d /tmp \
    && /tmp/aws/install \
    && rm -rf /tmp/awscliv2.zip /tmp/aws

# Install Google Cloud CLI via official apt repository
RUN curl -fsSL https://packages.cloud.google.com/apt/doc/apt-key.gpg \
      | gpg --dearmor -o /usr/share/keyrings/cloud.google.gpg \
    && echo "deb [signed-by=/usr/share/keyrings/cloud.google.gpg] https://packages.cloud.google.com/apt cloud-sdk main" \
      > /etc/apt/sources.list.d/google-cloud-sdk.list \
    && apt-get update -y \
    && apt-get install -y --no-install-recommends google-cloud-cli \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /kailua/out/kailua-cli /usr/local/bin/kailua-cli

ENTRYPOINT ["/bin/sh", "-c"]
