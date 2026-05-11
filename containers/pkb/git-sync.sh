#!/usr/bin/env bash
# git-sync.sh — Mechanical git sync for a single repo.
# Usage: git-sync.sh /path/to/repo
#
# Commits local changes, pulls, pushes.
# Exits non-zero on failure (P#8: no fallbacks).
#
# Vendored from nicsuzor/dotfiles scripts/git-sync.sh — keep in sync.

set -euo pipefail

REPO="${1:?Usage: git-sync.sh /path/to/repo}"
cd "$REPO"

# Recover from an interrupted rebase/merge (e.g., container killed mid-sync).
# Without this, a stuck .git/rebase-merge traps every subsequent run, the
# entrypoint exits non-zero under `set -e`, and Docker crash-loops the
# container — during which the background sync loop piles up commits on a
# detached HEAD that later get wiped by a panicked `git reset --hard`.
# Safe: auto-sync commits are mechanical snapshots, replayed from HEAD on pull.
if [ -d ".git/rebase-merge" ] || [ -d ".git/rebase-apply" ]; then
    echo "[git-sync] WARNING: interrupted rebase detected; aborting" >&2
    git rebase --abort 2>/dev/null || git reset --merge 2>/dev/null || true
    # Last-ditch: if metadata is corrupt (e.g. invalid 'onto'), `--abort`
    # refuses and the state dirs persist — trapping the next run. Removing
    # them manually is safe here: auto-sync commits are mechanical snapshots
    # that will be re-derived on the next pull.
    rm -rf ".git/rebase-merge" ".git/rebase-apply"
fi
if [ -f ".git/MERGE_HEAD" ]; then
    echo "[git-sync] WARNING: interrupted merge detected; aborting" >&2
    git merge --abort 2>/dev/null || git reset --merge 2>/dev/null || true
    rm -f ".git/MERGE_HEAD"
fi

# Commit any local changes
git add -A
if ! git diff --cached --quiet; then
    git commit -m "auto: sync $(date -u +%Y-%m-%d\ %H:%M)"
fi

# Merge rather than rebase: a killed merge is cleanly recoverable by the
# preflight above, and `ort` auto-resolves conflicts between mechanical
# auto-sync snapshots — whereas partial-rebase replay state does not.
git pull --no-rebase --no-edit
git push
