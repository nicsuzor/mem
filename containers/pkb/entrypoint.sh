#!/bin/bash
set -eo pipefail

# XDG_CACHE_HOME is set as an ENV in the Dockerfile so it applies to
# `docker exec` sessions too (the earlier shell export only covered
# entrypoint children, causing every exec to re-download BGE-M3).

# Fail fast when BRAIN_REPO_URL is missing or unreachable. The fail-fast
# contract is verified by the negative test in scripts/pkb-negative-test.sh.
if [ -z "${BRAIN_REPO_URL:-}" ]; then
    echo "FATAL: BRAIN_REPO_URL is unset — refusing to start in degraded state" >&2
    exit 1
fi

# Configure git auth via Docker secret (no PAT in remote URL or .git/config).
# Rotation = overwrite /run/secrets/gh_pat on the host and restart the container.
if [ -f /run/secrets/gh_pat ]; then
    GIT_USER="${GIT_USER:-botnicbot}"
    git config --global credential.helper \
        "!f() { echo username=${GIT_USER}; echo password=$(cat /run/secrets/gh_pat); }; f"
    # Strip any embedded token from existing remotes (migration from earlier setup
    # where the PAT was baked into BRAIN_REPO_URL). Idempotent.
    if [ -d "$ACA_DATA/.git" ]; then
        current_url=$(git -C "$ACA_DATA" remote get-url origin 2>/dev/null || true)
        clean_url=$(printf '%s' "$current_url" | sed -E 's#https://[^@/]+@#https://#')
        if [ -n "$clean_url" ] && [ "$current_url" != "$clean_url" ]; then
            echo "Stripping embedded credentials from origin URL"
            git -C "$ACA_DATA" remote set-url origin "$clean_url"
        fi
    fi
else
    echo "WARNING: /run/secrets/gh_pat not present — git operations will use BRAIN_REPO_URL credentials" >&2
fi

# PKB_ROLE selects between the MCP server (default) and a sync-only sidecar.
# The sidecar (PKB_ROLE=sync) shares the brain volume with the server and
# runs the periodic git-sync loop, decoupling sync failures from the
# server's lifecycle. See dotfiles compose `pkb-sync` service.
PKB_ROLE="${PKB_ROLE:-server}"

if [ "$PKB_ROLE" = "sync" ]; then
    # Sidecar: wait for the server to have cloned the brain repo, then
    # loop git-sync forever. The server owns first-boot clone semantics
    # (it does the startup sync below in server mode) — the sidecar only
    # takes over the ongoing periodic sync.
    while [ ! -d "$ACA_DATA/.git" ]; do
        echo "[pkb-sync] waiting for pkb server to initialise $ACA_DATA ..."
        sleep 5
    done

    # Register the task-yaml merge driver in this clone too — git-sync.sh's
    # pull invokes it, and merge.<name>.driver is per-clone config.
    git -C "$ACA_DATA" config merge.task-yaml.driver \
        './.git-merge-task-yaml.sh %O %A %B %P'

    SYNC_PERIOD="${SYNC_PERIOD:-60}"
    MAX_FAILURES="${SYNC_MAX_FAILURES:-5}"
    FAIL_COUNT=0
    echo "[pkb-sync] starting loop (period=${SYNC_PERIOD}s, max_failures=${MAX_FAILURES})"
    while true; do
        sleep "$SYNC_PERIOD"
        if /usr/local/bin/git-sync.sh "$ACA_DATA" 2>&1 | sed 's/^/[git-sync] /'; then
            FAIL_COUNT=0
        else
            FAIL_COUNT=$((FAIL_COUNT + 1))
            echo "[git-sync] ERROR: sync failed ($FAIL_COUNT/$MAX_FAILURES) — DATA AT RISK" >&2
            if [ "$FAIL_COUNT" -ge "$MAX_FAILURES" ]; then
                echo "[git-sync] FATAL: $MAX_FAILURES consecutive sync failures — exiting" >&2
                exit 1
            fi
        fi
    done
fi

# --- server role (default) ---

# Clone brain repo if not already present
if [ ! -d "$ACA_DATA/.git" ]; then
    echo "Cloning brain repo from $BRAIN_REPO_URL ..."
    if ! git clone "$BRAIN_REPO_URL" "$ACA_DATA"; then
        echo "FATAL: git clone failed for BRAIN_REPO_URL=$BRAIN_REPO_URL — refusing to start" >&2
        exit 1
    fi
fi

# Register the task-yaml merge driver. Lives in the brain repo
# (.gitattributes + .git-merge-task-yaml.sh). The driver attribute is
# set in .gitattributes; the executable mapping is per-clone and must
# be configured here so git-sync.sh's pull can invoke it.
git -C "$ACA_DATA" config merge.task-yaml.driver \
    './.git-merge-task-yaml.sh %O %A %B %P'

# Sync on startup — commits any stranded changes from prior run, pulls, pushes.
# If this fails, container does not start. (P#8: no fallbacks)
# Kept in server mode for clean first-boot semantics; ongoing sync is owned
# by the pkb-sync sidecar (PKB_ROLE=sync).
echo "Syncing brain repo on startup..."
/usr/local/bin/git-sync.sh "$ACA_DATA"

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
