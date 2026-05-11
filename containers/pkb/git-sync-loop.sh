#!/bin/bash
set -eo pipefail

# Entrypoint for the pkb-sync sidecar — sole owner of git operations
# against the shared brain volume. Clones on first boot, commits/pulls/
# pushes on startup, then loops forever.
#
# Decoupled from the MCP server so sync failures don't kill the server
# and a server restart doesn't restart the sync loop unnecessarily.
# See spike aops-aeb70559.

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
echo "Syncing brain repo on startup..."
/usr/local/bin/git-sync.sh "$ACA_DATA"

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
