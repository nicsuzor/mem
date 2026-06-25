#!/usr/bin/env bash
# push-vectors.sh — ship a Windows-built vector store into the services-new pkb container.
#
# WHY THIS EXISTS
#   Reindexing ~8k docs is a heavy batch job: ≈2h on Windows GPU, days on CPU,
#   and slow on WSL2 (paravirtualised GPU). So we reindex on Windows (the fast
#   path) and ship the resulting pkb_vectors.bin to the container, instead of
#   letting the container reindex itself from scratch on CPU.
#
#   The container's brain markdown is LF (Linux git clone). As of the CRLF->LF
#   hash-normalization fix in src/pkb.rs, a .bin built by a FIXED Windows binary
#   stores line-ending-independent hashes that match the container's LF files.
#   That means the container's startup `pkb reindex` sees everything as fresh and
#   only re-embeds docs that genuinely changed since the Windows reindex — cheap.
#
#   >>> Precondition: the .bin MUST be built by a pkb binary that includes the
#       hash-normalization fix. If you ship a .bin from an OLD Windows binary,
#       the verify step below will report a high stale count and you should NOT
#       trust it (the container would re-embed everything on CPU).
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
set -euo pipefail

SRC_BIN="${1:-${SRC_BIN:-/home/nic/nicsu/brain/pkb_vectors.bin}}"
REMOTE="${REMOTE:-services-new}"
CONTAINER="${CONTAINER:-pkb}"
REMOTE_DB="${REMOTE_DB:-/data/brain/pkb_vectors.bin}"
STALE_OK="${STALE_OK:-50}"

STAGE="/tmp/pkb_vectors.bin.incoming"          # staging path on the remote host
log() { printf '\033[36m[push-vectors]\033[0m %s\n' "$*"; }
die() { printf '\033[31m[push-vectors] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

# 1. Validate source
[ -f "$SRC_BIN" ] || die "source not found: $SRC_BIN"
SIZE=$(stat -c%s "$SRC_BIN" 2>/dev/null || stat -f%z "$SRC_BIN")
[ "$SIZE" -gt 1000000 ] || die "source suspiciously small (${SIZE} bytes): $SRC_BIN"
log "source: $SRC_BIN ($(( SIZE / 1024 / 1024 )) MB)"

# 2. Transfer to the remote host (resumable, compressed)
log "rsync -> $REMOTE:$STAGE"
rsync -avP --inplace "$SRC_BIN" "$REMOTE:$STAGE"

# 3. Land it in the container volume and atomically swap (same fs => atomic rename).
#    Back up the current store first so we can roll back if verify fails.
log "staging into container $CONTAINER and swapping atomically"
ssh "$REMOTE" 'bash -s' -- "$CONTAINER" "$REMOTE_DB" "$STAGE" <<'REMOTE_EOF'\nset -euo pipefail\nCONTAINER="$1"\nREMOTE_DB="$2"\nSTAGE="$3"\ntrap 'rm -f "$STAGE"' EXIT\ndir=$(dirname "$REMOTE_DB"); base=$(basename "$REMOTE_DB")\ndocker cp "$STAGE" "$CONTAINER:$dir/$base.incoming"\ndocker exec "$CONTAINER" sh -c '\n  set -e\n  cd "'"$dir"'"\n  [ -f "'"$base"'" ] && cp -f "'"$base"'" "'"$base"'.bak"\n  mv "'"$base"'.incoming" "'"$base"'"\n'\nREMOTE_EOF

# 4. Restart ONLY the pkb server so it reloads the store from disk.
#    The pkb-sync sidecar is a separate service — leave it running.
log "restarting $CONTAINER"
ssh "$REMOTE" "docker restart $CONTAINER >/dev/null"

# 5. Verify: scan + hash the brain against the new store. A low stale count proves
#    the hashes matched (cheap startup reindex). A high count means the .bin was
#    NOT built with the hash-fix binary — roll back rather than trigger a CPU reindex.
log "waiting for server, then checking staleness"
sleep 8
STATUS=$(ssh "$REMOTE" "docker exec $CONTAINER pkb status" 2>/dev/null || true)
echo "$STATUS" | sed 's/^/    /'
STALE=$(printf '%s\n' "$STATUS" | grep -oiE '[0-9]+ document\(s\) need re-indexing' | grep -oE '^[0-9]+' || echo 0)

if [ "${STALE:-0}" -gt "$STALE_OK" ]; then
  die "stale=$STALE > $STALE_OK — hashes did NOT match. The .bin was likely built
       by an OLD (pre-fix) binary. Roll back with:
         ssh $REMOTE 'docker exec $CONTAINER sh -c \"cd $(dirname "$REMOTE_DB") && mv $(basename "$REMOTE_DB").bak $(basename "$REMOTE_DB")\" && docker restart $CONTAINER'"
fi

log "done — stale=$STALE (<= $STALE_OK). Store is live."
