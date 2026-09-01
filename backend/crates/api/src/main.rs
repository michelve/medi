//! `medi` — axum HTTP + HLS server entrypoint.
//!
//! Thin wrapper over the `medi_api` library: boots configuration, tracing, the
//! SQLite database, and the catalog response cache, then serves the full route
//! surface from `docs/.tasks/02-api-contract.md`. Catalog routes are live;
//! stream/HLS and asset generation are wired in Phases 2–3.

use std::sync::Arc;

use medi_api::cache::ResponseCache;
use medi_api::{router, AppState};
use medi_core::AppConfig;
use medi_ingest::{Invalidator, WorkerConfig};

/// Default catalog cache capacity (distinct cached responses). Catalog URLs are few
/// — a handful of library pages plus per-title detail docs — so this comfortably
/// holds the hot set; the LRU evicts the rest.
const CACHE_CAPACITY: u64 = 4096;

/// Concurrent transcode-session cap, from host capability (`docs/.tasks/20` §Scaling:
/// UHD 770 ≈ 4–7 4K streams, Arc A380 ≈ 8–12). Overridable via `MAX_TRANSCODES`.
fn max_transcode_sessions(caps: &medi_transcode::HwCaps) -> usize {
    if let Some(n) = std::env::var("MAX_TRANSCODES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        return n.max(1);
    }
    // A conservative default for an iGPU; a discrete Arc/NVENC box can raise it.
    match caps.vendor {
        Some(_) => 4,
        None => 1, // software-only: one transcode saturates a CPU.
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Env-driven config (Phase 1 sub-task 1): MEDIA_DIR, CONFIG_DIR, BIND_ADDR,
    // DB_POOL_SIZE, OFFPEAK_*; each falls back to its default.
    let config = AppConfig::from_env()?;
    tracing::info!(
        media_dir = %config.media_dir.display(),
        config_dir = %config.config_dir.display(),
        "resolved configuration",
    );

    // Open (creating if absent) and migrate the database, then build shared state.
    let db = medi_db::open(config.db_path(), config.db_pool_size)?;
    let cache = ResponseCache::new(CACHE_CAPACITY);
    let bind_addr = config.bind_addr.clone();
    let media_dir = config.media_dir.clone();
    let images_dir = config.images_dir();

    // Auto-seed the default movie/series libraries rooted at MEDIA_DIR on first boot
    // (`docs/.tasks/60` Phase B) so an existing single-`/media` deployment keeps working
    // with no config change. Idempotent — a no-op once libraries exist.
    {
        let seed_db = db.clone();
        let media = media_dir.to_string_lossy().into_owned();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        tokio::task::spawn_blocking(move || {
            let conn = seed_db.conn()?;
            medi_db::writes::seed_default_libraries(&conn, &media, now)
        })
        .await??;
    }

    // Build the metadata provider from config (`docs/.tasks/60` Phase A). `None` when
    // metadata is disabled or no key is set — ingest then runs filename-only with no
    // error (graceful degradation). When present, an `EnrichContext` is shared by the
    // ingest worker (auto-enrichment) and the API's manual metadata endpoints.
    let enrich_ctx = medi_metadata::build_provider(&config).and_then(|provider| {
        match medi_metadata::HttpFetcher::new() {
            Ok(fetcher) => Some(medi_metadata::EnrichContext {
                db: db.clone(),
                provider,
                fetcher: std::sync::Arc::new(fetcher),
                images_dir: images_dir.clone(),
            }),
            Err(err) => {
                tracing::warn!(error = %err, "failed to build image fetcher; enrichment disabled");
                None
            }
        }
    });

    // Probe host HWA capabilities (Phase 2) and build the transcode session manager.
    // A GPU-less host yields software-only caps, so `/api/stream` always has a target.
    let caps = medi_transcode::caps::probe().await;
    let transcode = medi_transcode::SessionManager::new(
        config.config_dir.join("hls"),
        max_transcode_sessions(&caps),
        caps.clone(),
    );
    transcode.spawn_reaper();

    // Keep a clone of the caps for the background asset worker (below) before `caps` is
    // moved into `AppState`.
    let asset_caps = caps.clone();

    let mut state = AppState::new(db.clone(), cache.clone(), config, transcode, caps);
    if let Some(ctx) = &enrich_ctx {
        state = state.with_enrichment(ctx.clone());
    }

    // Ingestion worker (Phase 1 sub-tasks 3–5,7). The API cache's invalidate_all is
    // passed as the opaque callback so ingest can flush the catalog after a write
    // without depending on the `api` crate.
    let invalidate: Invalidator = Arc::new(move || cache.invalidate_all());
    let mut worker_cfg = WorkerConfig::new(media_dir.clone());
    if let Some(ctx) = enrich_ctx.clone() {
        // Auto-enrichment (`docs/.tasks/60` Phase A): a scan that writes new titles
        // enriches everything still pending, so dropping a file into a watched folder
        // fetches its metadata with no manual step.
        worker_cfg = worker_cfg.with_enrichment(ctx);
    }
    if media_dir.is_dir() {
        // Kick off an initial scan in the background, then keep watching for changes,
        // so the HTTP server starts serving immediately (health check stays green on a
        // large first scan).
        let bg_db = db.clone();
        let bg_cfg = worker_cfg.clone();
        let bg_invalidate = invalidate.clone();
        tokio::spawn(async move {
            if let Err(err) = medi_ingest::run_scan(&bg_db, &bg_cfg, &bg_invalidate).await {
                tracing::error!(error = %err, "initial ingest scan failed");
            }
            if let Err(err) = medi_ingest::watch(bg_db, bg_cfg, bg_invalidate).await {
                tracing::error!(error = %err, "media watcher stopped");
            }
        });
    } else {
        tracing::warn!(
            media_dir = %media_dir.display(),
            "MEDIA_DIR does not exist; skipping ingestion (catalog will be empty)",
        );
    }

    // Background asset worker (Phase 3, `docs/.tasks/30`): generates 720p hover previews
    // + trickplay sprites under `/config`, gated to the off-peak window and yielding the
    // GPU to live transcode sessions. Shares the config + session manager from state so
    // its GPU-idle guard sees the same live sessions `/api/stream` starts.
    {
        let scheduler = medi_assets::Scheduler::new(state.config.clone(), state.transcode.clone());
        let asset_cfg = medi_assets::AssetWorkerConfig::new(asset_caps);
        let asset_db = db.clone();
        tokio::spawn(async move {
            if let Err(err) = medi_assets::run(asset_db, scheduler, asset_cfg).await {
                tracing::error!(error = %err, "asset worker stopped");
            }
        });
    }

    // Artwork orphan sweep (`docs/.tasks/60` §Orphan reaping): a low-frequency backstop
    // that reconciles `/config/images` against surviving title ids, reclaiming art left
    // behind by a reap or a manual DB edit. Runs off the request path.
    {
        let sweep_db = db.clone();
        let sweep_images = images_dir.clone();
        tokio::spawn(async move {
            // First sweep shortly after boot, then hourly.
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                interval.tick().await;
                if let Err(err) =
                    medi_metadata::sweep_orphan_images(&sweep_db, &sweep_images).await
                {
                    tracing::warn!(error = %err, "periodic artwork sweep failed");
                }
            }
        });
    }

    let app = router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(addr = %bind_addr, "medi listening");
    axum::serve(listener, app).await?;
    Ok(())
}
