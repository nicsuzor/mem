# PKB MCP server image — published to ghcr.io/nicsuzor/pkb
#
# Runtime-only image. The pkb binary comes from a published GitHub release
# (built by .github/workflows/build-release.yml) — no Rust toolchain in
# this build context. Pass the release asset URL via build-arg:
#
#   docker build --build-arg PKB_BINARY_URL=https://github.com/nicsuzor/mem/releases/download/<tag>/mem-<tag>-x86_64-linux.tar.gz .
#
# The publish-image.yml workflow sets this automatically from the latest
# release on release.published events.
#
# Runtime requirements (set by the docker-compose unit on services-new):
#   - ACA_DATA           where the brain repo lives (default: /data/brain)
#   - BRAIN_REPO_URL     https URL to the brain repo (required)
#   - GIT_USER           git username for credential helper (default: botnicbot)
#   - /run/secrets/gh_pat  Docker secret containing the GitHub PAT
#
# Exposes :8026 (MCP HTTP). See containers/pkb/entrypoint.sh for the
# full runtime contract (allowed hosts, reindex policy, sync loop).

FROM debian:bookworm-slim

ARG PKB_BINARY_URL

RUN apt-get update && apt-get install -y --no-install-recommends \
    git ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# Git identity for auto-commits performed by git-sync.sh
RUN git config --global user.name "pkb-sync" && \
    git config --global user.email "pkb-sync@noreply"

RUN test -n "$PKB_BINARY_URL" || { echo "ERROR: PKB_BINARY_URL build-arg required" >&2; exit 1; } && \
    curl -fsSL "$PKB_BINARY_URL" | tar -xz -C /usr/local/bin pkb && \
    chmod +x /usr/local/bin/pkb

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
