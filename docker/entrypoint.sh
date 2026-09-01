#!/usr/bin/env bash
# medi container entrypoint (Phase 5, docs/.tasks/50).
#
# The `medi` binary opens+migrates SQLite itself on boot (medi_db::open runs the
# refinery migrations), so there is no separate migrate step here. This script
# just prepares the writable appdata layout, logs what hardware acceleration the
# host actually exposed, then hands PID 1 to the server via `exec`.
set -euo pipefail

CONFIG_DIR="${CONFIG_DIR:-/config}"
MEDIA_DIR="${MEDIA_DIR:-/media}"

log() { printf '[entrypoint] %s\n' "$*"; }

# --- Writable appdata layout ----------------------------------------------
# These mirror the paths medi_core::AppConfig derives under CONFIG_DIR
# (library.db, previews/, trickplay/, images/, hls/). Pre-creating them keeps the
# first-run logs clean and surfaces a bad /config mount immediately.
mkdir -p \
  "${CONFIG_DIR}" \
  "${CONFIG_DIR}/previews" \
  "${CONFIG_DIR}/trickplay" \
  "${CONFIG_DIR}/images" \
  "${CONFIG_DIR}/hls"

if [ ! -w "${CONFIG_DIR}" ]; then
  log "FATAL: CONFIG_DIR '${CONFIG_DIR}' is not writable. Check the /config volume mapping."
  exit 1
fi

# --- Read-only media sanity (non-fatal) -----------------------------------
if [ ! -d "${MEDIA_DIR}" ]; then
  log "WARN: MEDIA_DIR '${MEDIA_DIR}' does not exist; the catalog will be empty until it is mounted."
fi

# --- Hardware acceleration visibility (informational) ---------------------
# The server probes this itself (medi_transcode::caps), but logging it here makes
# a misconfigured passthrough obvious in the container's first lines.
if [ -d /dev/dri ]; then
  log "GPU: /dev/dri present ($(ls /dev/dri 2>/dev/null | tr '\n' ' '))"
  if command -v vainfo >/dev/null 2>&1; then
    vainfo 2>/dev/null | grep -E 'Driver version|VAProfile' | head -n 3 | while read -r line; do
      log "GPU: ${line}"
    done || true
  fi
else
  log "GPU: no /dev/dri — Intel/AMD QSV/VA-API unavailable (pass --device /dev/dri to enable)."
fi

if command -v nvidia-smi >/dev/null 2>&1; then
  log "GPU: NVIDIA runtime detected ($(nvidia-smi --query-gpu=name --format=csv,noheader 2>/dev/null | head -n1))"
else
  log "GPU: no NVIDIA runtime (use --runtime=nvidia on NVIDIA hosts)."
fi

log "starting medi on ${BIND_ADDR:-0.0.0.0:8096} (media=${MEDIA_DIR} config=${CONFIG_DIR})"

# exec so `medi` becomes PID 1 and receives SIGTERM directly (clean shutdown).
exec /usr/local/bin/medi
