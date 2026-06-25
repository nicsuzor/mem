#!/usr/bin/env bash
# push-vectors.sh — ship a Windows-built vector store into the services-new pkb container.
#
# WHY THIS EXISTS
#   Reindexing ~8k docs is a heavy batch job: ≈2h on Windows GPU, days on CPU,
#   and slow on WSL2 (paravirtualised GPU). So we reindex on Windows (the fast
#   path) and ship the resulting pkb_vectors.bin to the container, instead of
#   letting the container reindex itself from scratch on CPU.
#
# TWO PRECONDITIONS — BOTH REQUIRED (learned the hard way 2026-06-25):
#   1. SERIALIZATION MATCH. The .bin is bincode; its on-disk format is tied to
#      the exact pkb build (e.g. the PathBuf-serialization change in 88abde7).
#      The container's pkb binary MUST be built from the SAME commit as the
#      binary that produced the .bin, or the container fails to deserialize and
#      silently falls back to an EMPTY store (Documents: 0) — worse than before.
#      This script checks `pkb --version` on both sides and refuses on mismatch.
#   2. LINE-ENDING-INDEPENDENT HASHES. The .bin must be built by a binary with
#      the CRLF->LF hash fix (src/pkb.rs), else the container (LF git clone)
#      reads every doc stale and re-embeds everything on CPU.
#
# USAGE
#   scripts/push-vectors.sh [SRC_BIN]
#   SRC_BIN   path to the freshly-built pkb_vectors.bin
#             (default: $SRC_BIN env, else the Windows brain mount below)
#
# OVERRIDABLE ENV
#   REMOTE        ssh host running docker            (default: services-new)
#   CONTAINER     pkb server container/service name  (default: pkb)
#   REMOTE_DB     vector store path inside container (default: /data/brain/pkb_vectors.bin)
#   STALE_OK      max acceptable stale docs post-swap (default: 50)
#   SRC_VERSION   expected `pkb --version` of the binary that built SRC_BIN.
#                 If set, must match the container's version exactly. Bypass the
#                 hard version gate with SKIP_VERSION_CHECK=1 (not recommended).
set -euo pipefail

SRC_BIN="${1:-${SRC_BIN:-/home/nic/nicsu/brain/pkb_vectors.bin}}"
REMOTE="${REMOTE:-services-new}"
CONTAINER="${CONTAINER:-pkb}"
REMOTE_DB="${REMOTE_DB:-/data/brain/pkb_vectors.bin}"
STALE_OK="${STALE_OK:-50}"

STAGE="/tmp/pkb_vectors.bin.incoming"          # staging path on the remote host
log() { printf '\033[36m[push-vectors]\033[0m %s\n' "$*"; }
die() { printf '\033[31m[push-vectors] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

rollback() {
  log "rolling back to previous store (.bak) and restarting"
  ssh "$REMOTE" "docker exec $CONTAINER sh -c 'cd $(dirname "$REMOTE_DB") && [ -f $(basename "$REMOTE_DB").bak ] && mv -f $(basename "$REMOTE_DB").bak $(basename "$REMOTE_DB")' && docker restart $CONTAINER >/dev/null" \
    && log "rollback complete" || log "rollback FAILED — inspect $CONTAINER manually"
}

# 1. Validate source
[ -f "$SRC_BIN" ] || die "source not found: $SRC_BIN"
SIZE=$(wc -c < "$SRC_BIN")
[ "$SIZE" -gt 1000000 ] || die "source suspiciously small (${SIZE} bytes): $SRC_BIN"
log "source: $SRC_BIN ($(( SIZE / 1024 / 1024 )) MB)"

# 2. PREFLIGHT: serialization compatibility. The container's pkb build must match
#    the one that produced SRC_BIN, or deserialize fails -> empty store.
CVER=$(ssh "$REMOTE" "docker exec $CONTAINER pkb --version" 2>/dev/null | tr -d '\r' || true)
[ -n "$CVER" ] || die "could not read container pkb --version (is $CONTAINER up?)"
log "container binary: $CVER"
if [ "${SKIP_VERSION_CHECK:-0}" != "1" ] && [ -n "${SRC_VERSION:-}" ]; then
  if [ "$SRC_VERSION" != "$CVER" ]; then
    die "version mismatch — SRC_BIN built by '$SRC_VERSION' but container runs '$CVER'.
       bincode formats differ across builds; the container would read an EMPTY store.
       Rebuild/redeploy the container image from the SAME commit as the Windows binary,
       or re-export SRC_BIN from a binary matching the container. (SKIP_VERSION_CHECK=1 to override.)"
  fi
  log "version match confirmed"
else
  log "WARNING: version not verified (set SRC_VERSION to enforce). The post-swap"
  log "         Documents:0 guard below is the backstop against a format mismatch."
fi

# 3. Transfer to the remote host (resumable)
log "rsync -> $REMOTE:$STAGE"
rsync -avP --inplace "$SRC_BIN" "$REMOTE:$STAGE"

# 4. Land it in the container volume and atomically swap (same fs => atomic rename).
#    Back up the current store first so we can roll back if verify fails.
log "staging into container $CONTAINER and swapping atomically"
ssh "$REMOTE" 'bash -s' -- "$CONTAINER" "$REMOTE_DB" "$STAGE" <<'REMOTE_EOF'
set -euo pipefail
CONTAINER="$1"; REMOTE_DB="$2"; STAGE="$3"
trap 'rm -f "$STAGE"' EXIT
dir=$(dirname "$REMOTE_DB"); base=$(basename "$REMOTE_DB")
docker cp "$STAGE" "$CONTAINER:$dir/$base.incoming"
docker exec "$CONTAINER" sh -c '
  set -e
  cd "'"$dir"'"
  [ -f "'"$base"'" ] && cp -f "'"$base"'" "'"$base"'.bak"
  mv "'"$base"'.incoming" "'"$base"'"
'
REMOTE_EOF

# 5. Restart ONLY the pkb server so it reloads the store from disk.
#    The pkb-sync sidecar is a separate service — leave it running.
log "restarting $CONTAINER"
ssh "$REMOTE" "docker restart $CONTAINER >/dev/null"

# 6. Verify. Wait for the server, then check BOTH:
#    - Documents > 0      => the store actually deserialized (catches format mismatch)
#    - stale <= STALE_OK  => hashes matched (catches a non-fixed / line-ending .bin)
log "waiting for server to be ready..."
STATUS=""
for _ in $(seq 1 15); do
  STATUS=$(ssh "$REMOTE" "docker exec $CONTAINER pkb status" 2>/dev/null) && [ -n "$STATUS" ] && break
  sleep 1
done
[ -n "$STATUS" ] || { die "no status from container — server may have crashed"; }
echo "$STATUS" | sed 's/^/    /'

DOCS=$(printf '%s\n' "$STATUS" | grep -oiE 'Documents:[[:space:]]*[0-9]+' | grep -oE '[0-9]+' | head -n1)
DOCS="${DOCS:-0}"
STALE=$(printf '%s\n' "$STATUS" | grep -oiE '[0-9]+ document\(s\) need re-indexing' | grep -oE '^[0-9]+' | head -n1)
STALE="${STALE:-0}"

if [ "$DOCS" -eq 0 ]; then
  rollback
  die "Documents:0 — the container could NOT deserialize the .bin (format/version
       mismatch: it fell back to an empty store). Rolled back to the previous .bin."
fi

if [ "$STALE" -gt "$STALE_OK" ]; then
  rollback
  die "stale=$STALE > $STALE_OK — hashes did not match (non-fixed or wrong-line-ending
       .bin). Rolled back. Set STALE_OK high to push anyway."
fi

log "done — Documents=$DOCS, stale=$STALE (<= $STALE_OK). Store is live."
