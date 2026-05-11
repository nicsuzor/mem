# PKB MCP server image — published to ghcr.io/nicsuzor/pkb
#
# Build pipeline:
#   1. Rust builder stage compiles the `pkb` binary from this repo.
#   2. Slim Debian runtime stage carries git + curl + the binary +
#      git-sync.sh + entrypoint.sh.
#
# Runtime requirements (set by the docker-compose unit on services-new):
#   - ACA_DATA           where the brain repo lives (default: /data/brain)
#   - BRAIN_REPO_URL     https URL to the brain repo (required)
#   - GIT_USER           git username for credential helper (default: botnicbot)
#   - /run/secrets/gh_pat  Docker secret containing the GitHub PAT
#
# Exposes :8026 (MCP HTTP). See containers/pkb/entrypoint.sh for the
# full runtime contract (allowed hosts, reindex policy, sync loop).

FROM rust:1.88-bookworm AS builder

WORKDIR /build

# Cache cargo registry/git/target across builds via BuildKit cache mounts.
# Falls back to plain copy if BuildKit isn't enabled (no harm).
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY models ./models

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --bin pkb && \
    cp target/release/pkb /usr/local/bin/pkb

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    git ca-certificates curl jq \
    && rm -rf /var/lib/apt/lists/*

# Git identity for auto-commits performed by git-sync.sh
RUN git config --global user.name "pkb-sync" && \
    git config --global user.email "pkb-sync@noreply"

COPY --from=builder /usr/local/bin/pkb /usr/local/bin/pkb

COPY containers/pkb/git-sync.sh /usr/local/bin/git-sync.sh
COPY containers/pkb/entrypoint.sh /entrypoint.sh
RUN chmod +x /usr/local/bin/git-sync.sh /entrypoint.sh && \
    ACA_DATA=/tmp pkb --version

ENV ACA_DATA=/data/brain
# Cache the ONNX BGE-M3 model on the persistent volume so `docker exec pkb ...`
# doesn't re-download ~2GB on every invocation.
ENV XDG_CACHE_HOME=/data/cache
EXPOSE 8026

ENTRYPOINT ["/entrypoint.sh"]
