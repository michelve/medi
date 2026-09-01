# docker

Multi-stage, Debian-based image that compiles the Rust backend and injects the
proprietary user-mode GPU drivers (Intel media-va-driver + compute-runtime,
mesa-va-drivers). `entrypoint.sh` applies PRAGMAs/migrations on boot, then launches
the `medi` server. Authored in **Phase 5** (`docs/.tasks/50-phase5-playback-packaging.md`).
