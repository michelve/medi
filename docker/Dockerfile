# syntax=docker/dockerfile:1
#
# medi — multi-stage image (Phase 5, docs/.tasks/50-phase5-playback-packaging.md).
#
# Debian (not Alpine): proprietary GPU user-mode drivers and jellyfin-ffmpeg are
# built and packaged for glibc/Debian; musl support is a constant fight. The final
# image bakes in the Intel + AMD VA-API/QSV stack; NVIDIA is injected at runtime by
# the host's NVIDIA container runtime (--runtime=nvidia) and is NOT baked in.
#
# Build context is the repo root:  docker build -f docker/Dockerfile .

# ---------------------------------------------------------------------------
# 1) build — compile the release binary against Debian's glibc.
# ---------------------------------------------------------------------------
FROM rust:1-bookworm AS build
WORKDIR /src

# Copy the whole backend workspace. (A manifest-only pre-fetch layer is possible
# but brittle across a path-dependency workspace; a full copy keeps it simple and
# still caches on unchanged sources via BuildKit.)
COPY backend/ ./backend/

# `medi-db` uses rusqlite "bundled", so no system libsqlite is needed. Build the
# single `medi` binary from the api crate.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/backend/target \
    cd backend && cargo build --release --bin medi \
    && cp target/release/medi /usr/local/bin/medi

# ---------------------------------------------------------------------------
# 2) web — build the browser SPA (Task 80, docs/.tasks/80-web-ui-client.md).
# ---------------------------------------------------------------------------
# A clean `yarn install` runs in-image (the context ships source, not node_modules —
# see .dockerignore), then the `@medi/web` workspace is built to
# client/apps/web/dist. The runtime stage copies that dist to /usr/share/medi/web.
FROM node:22-bookworm AS web
WORKDIR /web
COPY client/ ./client/
RUN corepack enable \
    && cd client \
    && yarn install --frozen-lockfile \
    && yarn workspace @medi/web build

# ---------------------------------------------------------------------------
# 3) runtime — minimal Debian + GPU user-mode drivers + jellyfin-ffmpeg.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# -- Base + GPU user-mode drivers -------------------------------------------
# intel-media-va-driver-non-free : Intel iHD VA-API driver (QSV)         -> /dev/dri
# intel-opencl-icd               : Intel OpenCL (compute runtime) for DV tone-map
# mesa-va-drivers                : AMD/Mesa VA-API driver                 -> /dev/dri
# vainfo                         : lets `docker exec vainfo` verify passthrough
# curl                           : HEALTHCHECK probe
# non-free is needed for intel-media-va-driver-non-free (best QSV support).
RUN sed -i 's/ main$/ main contrib non-free non-free-firmware/' /etc/apt/sources.list.d/debian.sources \
    || sed -i 's/ main$/ main contrib non-free non-free-firmware/' /etc/apt/sources.list ; \
    apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates \
      curl \
      gnupg \
      intel-media-va-driver-non-free \
      intel-opencl-icd \
      mesa-va-drivers \
      vainfo \
    && rm -rf /var/lib/apt/lists/*

# -- jellyfin-ffmpeg (its own apt repo) -------------------------------------
# Installs to /usr/lib/jellyfin-ffmpeg/{ffmpeg,ffprobe}. The backend calls bare
# `ffmpeg`/`ffprobe` on PATH (see medi-transcode caps::ffmpeg_bin), so symlink them.
RUN install -d /etc/apt/keyrings \
    && curl -fsSL https://repo.jellyfin.org/jellyfin_team.gpg.key \
         | gpg --dearmor -o /etc/apt/keyrings/jellyfin.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/jellyfin.gpg] \
https://repo.jellyfin.org/debian bookworm main" > /etc/apt/sources.list.d/jellyfin.list \
    && apt-get update && apt-get install -y --no-install-recommends jellyfin-ffmpeg7 \
    && ln -sf /usr/lib/jellyfin-ffmpeg/ffmpeg  /usr/local/bin/ffmpeg \
    && ln -sf /usr/lib/jellyfin-ffmpeg/ffprobe /usr/local/bin/ffprobe \
    && rm -rf /var/lib/apt/lists/*

# -- The binary + web SPA + entrypoint ---------------------------------------
COPY --from=build /usr/local/bin/medi /usr/local/bin/medi
# Web assets ship IN the image (Task 80) — never under /config or /media. The binary
# serves them at `/` from WEB_DIR (medi_core::AppConfig::web_dir default).
COPY --from=web /web/client/apps/web/dist/ /usr/share/medi/web/
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
# Strip any CR (a Windows checkout can leave CRLF, which makes Linux look for a
# `bash\r` interpreter and crash-loop the container) then make it executable.
RUN sed -i 's/\r$//' /usr/local/bin/entrypoint.sh && chmod +x /usr/local/bin/entrypoint.sh

# Defaults mirror medi_core::AppConfig (all overridable via env / Unraid template).
#   MEDIA_DIR  read-only media source   CONFIG_DIR  read-write appdata (SQLite + assets)
ENV MEDIA_DIR=/media \
    CONFIG_DIR=/config \
    WEB_DIR=/usr/share/medi/web \
    BIND_ADDR=0.0.0.0:8096 \
    RUST_LOG=info \
    # NVIDIA container runtime reads these; harmless on non-NVIDIA hosts.
    NVIDIA_DRIVER_CAPABILITIES=compute,video,utility

VOLUME ["/config", "/media"]
EXPOSE 8096

# `medi` migrates SQLite itself on boot and starts serving immediately (health goes
# green even during a large first scan), so the probe just hits the liveness route.
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD curl -fsS http://localhost:8096/api/health || exit 1

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
