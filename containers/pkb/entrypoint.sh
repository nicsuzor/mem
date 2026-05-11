#!/bin/bash
set -eo pipefail

# XDG_CACHE_HOME is set as an ENV in the Dockerfile so it applies to
# `docker exec` sessions too (the earlier shell export only covered
# entrypoint children, causing every exec to re-download BGE-M3).

# All git operations against the brain volume are owned by the pkb-sync
# sidecar (git-sync-loop.sh). The server only reads the volume — it
# never clones, commits, pulls, or pushes. Wait for the sidecar to
# initialise the volume before starting.
while [ ! -d "$ACA_DATA/.git" ]; do
    echo "Waiting for pkb-sync sidecar to initialise $ACA_DATA ..."
    sleep 5
done

# Reindex in background so MCP serves immediately — otherwise reindex
# blocks the port, healthcheck fails after start_period, autoheal kills
# the container and reindex starts over in a loop.
#
# Concurrency: while reindex holds pkb_vectors.lock, mcp defers
# in-memory upserts (logging "Index locked by another process") and
# skips disk saves. When reindex releases the lock, mcp self-heals via
# maybe_drain_deferred (mcp_server.rs): reloads the store from disk and
# replays queued upserts. No restart required.
#
# Concurrency (-s2 -t2 -b8) on the 10GB-cap / 15GB-VM box: peak ~6GB
# RSS, ~3.5 cores, comfortable headroom. Drop to -s1 -t1 -b8 if the
# container limit ever shrinks back toward 4GB.
(
    sleep 5  # let mcp bind the port and start serving
    echo "[reindex] Starting async reindex..."
    if pkb reindex -s1 -t2 -b8; then
        echo "[reindex] Complete — mcp will reload from disk on next write"
    else
        echo "[reindex] FAILED — index remains stale" >&2
    fi
) &

# Start PKB
#
# --allowed-hosts: rmcp 1.5.0 default allowlists loopback only and 403s
# anything else. Setting --allowed-hosts REPLACES the default, so the
# list must include every name the server is reached as.
#
# Base list (always included): loopback + container DNS — covers
# healthchecks (curl localhost) and inter-container Docker DNS
# (overwhelm-dash -> pkb:8026).
#
# Deployment-specific names (e.g. external Tailscale hostnames) come
# in via PKB_EXTRA_HOSTS — comma-separated, set by the docker-compose
# unit. Bare hostnames match any port (rmcp's parse_allowed_authority).
ALLOWED_HOSTS="localhost,127.0.0.1,::1,pkb"
if [ -n "${PKB_EXTRA_HOSTS:-}" ]; then
    ALLOWED_HOSTS="${ALLOWED_HOSTS},${PKB_EXTRA_HOSTS}"
fi

exec pkb mcp --http --port 8026 --host 0.0.0.0 \
  --allowed-hosts "$ALLOWED_HOSTS"
