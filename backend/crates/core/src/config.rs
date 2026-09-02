//! Runtime configuration, resolved from the environment.
//!
//! Encodes the `/config` vs `/media` contract from `docs/.tasks/00-architecture.md`:
//! `media_dir` is read-only source, `config_dir` is the read-write appdata cache
//! (db, WAL, previews, trickplay, logs). Loaded once at boot in the `api` crate.

use figment::providers::{Env, Serialized};
use figment::Figment;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Read-only source library (Unraid array). Never written to.
    pub media_dir: PathBuf,
    /// Read-write appdata cache: library.db, previews, trickplay, logs.
    pub config_dir: PathBuf,
    /// Directory holding the built web SPA served at `/` (`docs/.tasks/80`).
    ///
    /// **Invariant:** web assets ship *in the image* (baked at build time), never under
    /// [`Self::config_dir`] (read-write appdata) or `media_dir` (read-only library). The
    /// default is the image path the Docker web stage copies `dist/` into.
    pub web_dir: PathBuf,
    /// Address the axum server binds to (LAN-first, no auth).
    pub bind_addr: String,
    /// r2d2 pool size; bound to a small multiple of CPU cores.
    pub db_pool_size: u32,
    /// Off-peak window (24h local hours) during which the assets worker may run.
    pub offpeak_start_hour: u8,
    pub offpeak_end_hour: u8,
    /// Max concurrent asset (preview/trickplay) ffmpeg jobs. Kept low so background
    /// asset generation stays subordinate to live transcodes (`docs/.tasks/30`).
    pub asset_max_concurrency: u32,

    // --- Metadata enrichment (`docs/.tasks/60`) ---------------------------------
    /// TMDB API key. Unset ⇒ the TMDB provider is unavailable and (if it is the
    /// selected provider) enrichment is silently skipped — graceful degradation.
    pub tmdb_api_key: Option<String>,
    /// OMDb API key. Unset ⇒ the OMDb provider is unavailable.
    pub omdb_api_key: Option<String>,
    /// Which provider to use: `"tmdb"` (default) or `"omdb"`.
    pub metadata_provider: String,
    /// Master switch for enrichment. `false` ⇒ ingest behaves filename-only even with
    /// a key configured.
    pub metadata_enabled: bool,
    /// Language tag passed to the provider (e.g. `"en-US"`) for overviews/titles.
    pub metadata_language: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            media_dir: PathBuf::from("/media"),
            config_dir: PathBuf::from("/config"),
            web_dir: PathBuf::from("/usr/share/medi/web"),
            bind_addr: "0.0.0.0:8096".to_string(),
            db_pool_size: 8,
            offpeak_start_hour: 2,
            offpeak_end_hour: 6,
            asset_max_concurrency: 1,
            tmdb_api_key: None,
            omdb_api_key: None,
            metadata_provider: "tmdb".to_string(),
            metadata_enabled: true,
            metadata_language: "en-US".to_string(),
        }
    }
}

impl AppConfig {
    /// Load configuration from the environment, over the [`Default`] baseline.
    ///
    /// The `/config` vs `/media` contract (`docs/.tasks/00-architecture.md`) plus the
    /// Phase 1 sub-task 1 keys. Each env var overrides the corresponding default;
    /// anything unset keeps its default, so a bare `docker run` with the standard
    /// `/media` + `/config` mounts needs no configuration at all.
    ///
    /// | Env var              | Field                | Default          |
    /// |----------------------|----------------------|------------------|
    /// | `MEDIA_DIR`          | `media_dir`          | `/media`         |
    /// | `CONFIG_DIR`         | `config_dir`         | `/config`        |
    /// | `WEB_DIR`            | `web_dir`            | `/usr/share/medi/web` |
    /// | `BIND_ADDR`          | `bind_addr`          | `0.0.0.0:8096`   |
    /// | `DB_POOL_SIZE`       | `db_pool_size`       | `8`              |
    /// | `OFFPEAK_START_HOUR` | `offpeak_start_hour` | `2`              |
    /// | `OFFPEAK_END_HOUR`   | `offpeak_end_hour`   | `6`              |
    /// | `ASSET_MAX_CONCURRENCY` | `asset_max_concurrency` | `1`        |
    /// | `TMDB_API_KEY`       | `tmdb_api_key`       | *(unset)*        |
    /// | `OMDB_API_KEY`       | `omdb_api_key`       | *(unset)*        |
    /// | `METADATA_PROVIDER`  | `metadata_provider`  | `tmdb`           |
    /// | `METADATA_ENABLED`   | `metadata_enabled`   | `true`           |
    /// | `METADATA_LANGUAGE`  | `metadata_language`  | `en-US`          |
    ///
    /// A `MEDI_`-prefixed form of each key is also accepted (e.g. `MEDI_BIND_ADDR`)
    /// so the vars can be namespaced when the container shares an environment.
    pub fn from_env() -> Result<Self, Error> {
        // Only our own keys — restrict the raw provider so unrelated process env
        // (PATH, HOME, …) never leaks into extraction.
        const KEYS: &[&str] = &[
            "media_dir",
            "config_dir",
            "web_dir",
            "bind_addr",
            "db_pool_size",
            "offpeak_start_hour",
            "offpeak_end_hour",
            "asset_max_concurrency",
            "tmdb_api_key",
            "omdb_api_key",
            "metadata_provider",
            "metadata_enabled",
            "metadata_language",
        ];

        Figment::new()
            // Baseline: the compiled-in defaults.
            .merge(Serialized::defaults(AppConfig::default()))
            // Bare env keys map 1:1 to the struct fields (MEDIA_DIR → media_dir, …).
            .merge(Env::raw().only(KEYS))
            // Optional namespaced overrides (MEDI_BIND_ADDR → bind_addr) win last.
            .merge(Env::prefixed("MEDI_").only(KEYS))
            .extract()
            .map_err(|e| Error::Config(e.to_string()))
    }

    /// Path to the SQLite database file (`/config/library.db`).
    pub fn db_path(&self) -> PathBuf {
        self.config_dir.join("library.db")
    }

    /// Directory holding the built web SPA (`docs/.tasks/80`). Served at `/` by the api
    /// crate; ships in the image, never under `config_dir`/`media_dir`.
    pub fn web_dir(&self) -> PathBuf {
        self.web_dir.clone()
    }

    /// Directory holding generated 720p hover previews.
    pub fn previews_dir(&self) -> PathBuf {
        self.config_dir.join("previews")
    }

    /// Directory holding generated trickplay sprites.
    pub fn trickplay_dir(&self) -> PathBuf {
        self.config_dir.join("trickplay")
    }

    /// Whether `hour` (0–23, local) falls inside the configured off-peak window during
    /// which the background assets worker may run (`docs/.tasks/30` §Off-peak).
    ///
    /// The window is `[start, end)` and wraps past midnight when `start > end` (e.g.
    /// `22`→`5`). `start == end` is treated as "always open" so an operator can disable
    /// the gate by setting both to the same hour.
    pub fn in_offpeak_window(&self, hour: u8) -> bool {
        let (start, end) = (self.offpeak_start_hour % 24, self.offpeak_end_hour % 24);
        if start == end {
            return true; // degenerate window ⇒ always in-window
        }
        if start < end {
            (start..end).contains(&hour)
        } else {
            // Wraps midnight: in-window if at/after start OR before end.
            hour >= start || hour < end
        }
    }

    /// Root for artwork (posters / backdrops) served by `GET /api/images/*path`.
    ///
    /// Artwork is downloaded by the metadata pipeline into `/config/images`; the
    /// `poster_path` / `backdrop_path` columns store paths relative to this root.
    pub fn images_dir(&self) -> PathBuf {
        self.config_dir.join("images")
    }

    /// The API key for the currently-selected [`Self::metadata_provider`], if one is
    /// configured. `None` here means "no provider available" ⇒ enrichment is skipped
    /// (graceful degradation, `docs/.tasks/60` §Requirements).
    pub fn active_metadata_key(&self) -> Option<&str> {
        match self.metadata_provider.to_ascii_lowercase().as_str() {
            "omdb" => self.omdb_api_key.as_deref(),
            // Default/unknown ⇒ TMDB (the documented default provider).
            _ => self.tmdb_api_key.as_deref(),
        }
        .filter(|k| !k.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_absent_yields_defaults() {
        figment::Jail::expect_with(|_jail| {
            let cfg = AppConfig::from_env().expect("defaults extract");
            assert_eq!(cfg.media_dir, PathBuf::from("/media"));
            assert_eq!(cfg.config_dir, PathBuf::from("/config"));
            assert_eq!(cfg.web_dir, PathBuf::from("/usr/share/medi/web"));
            assert_eq!(cfg.bind_addr, "0.0.0.0:8096");
            assert_eq!(cfg.db_pool_size, 8);
            assert_eq!(cfg.asset_max_concurrency, 1);
            // Metadata defaults: provider tmdb, enabled, en-US, no keys.
            assert_eq!(cfg.metadata_provider, "tmdb");
            assert!(cfg.metadata_enabled);
            assert_eq!(cfg.metadata_language, "en-US");
            assert!(cfg.tmdb_api_key.is_none());
            assert!(cfg.active_metadata_key().is_none(), "no key ⇒ enrichment off");
            Ok(())
        });
    }

    #[test]
    fn metadata_env_keys_load() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("TMDB_API_KEY", "tmdb-secret");
            jail.set_env("OMDB_API_KEY", "omdb-secret");
            jail.set_env("METADATA_PROVIDER", "omdb");
            jail.set_env("METADATA_ENABLED", "false");
            jail.set_env("METADATA_LANGUAGE", "fr-FR");

            let cfg = AppConfig::from_env().expect("env extract");
            assert_eq!(cfg.tmdb_api_key.as_deref(), Some("tmdb-secret"));
            assert_eq!(cfg.metadata_provider, "omdb");
            assert!(!cfg.metadata_enabled);
            assert_eq!(cfg.metadata_language, "fr-FR");
            // active key follows the selected provider.
            assert_eq!(cfg.active_metadata_key(), Some("omdb-secret"));
            Ok(())
        });
    }

    #[test]
    fn env_overrides_each_field() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("MEDIA_DIR", "/mnt/library");
            jail.set_env("CONFIG_DIR", "/var/medi");
            jail.set_env("WEB_DIR", "/opt/medi/web");
            jail.set_env("BIND_ADDR", "127.0.0.1:9000");
            jail.set_env("DB_POOL_SIZE", "16");
            jail.set_env("OFFPEAK_START_HOUR", "1");
            jail.set_env("OFFPEAK_END_HOUR", "7");
            jail.set_env("ASSET_MAX_CONCURRENCY", "3");

            let cfg = AppConfig::from_env().expect("env extract");
            assert_eq!(cfg.media_dir, PathBuf::from("/mnt/library"));
            assert_eq!(cfg.config_dir, PathBuf::from("/var/medi"));
            assert_eq!(cfg.web_dir, PathBuf::from("/opt/medi/web"));
            assert_eq!(cfg.bind_addr, "127.0.0.1:9000");
            assert_eq!(cfg.db_pool_size, 16);
            assert_eq!(cfg.offpeak_start_hour, 1);
            assert_eq!(cfg.offpeak_end_hour, 7);
            assert_eq!(cfg.asset_max_concurrency, 3);
            Ok(())
        });
    }

    #[test]
    fn offpeak_window_membership() {
        let mut cfg = AppConfig::default(); // 2..6
        assert!(!cfg.in_offpeak_window(1));
        assert!(cfg.in_offpeak_window(2));
        assert!(cfg.in_offpeak_window(5));
        assert!(!cfg.in_offpeak_window(6));

        // Wraps midnight: 22..5.
        cfg.offpeak_start_hour = 22;
        cfg.offpeak_end_hour = 5;
        assert!(cfg.in_offpeak_window(23));
        assert!(cfg.in_offpeak_window(0));
        assert!(cfg.in_offpeak_window(4));
        assert!(!cfg.in_offpeak_window(5));
        assert!(!cfg.in_offpeak_window(12));

        // Degenerate window (start == end) is always open.
        cfg.offpeak_start_hour = 3;
        cfg.offpeak_end_hour = 3;
        assert!(cfg.in_offpeak_window(3));
        assert!(cfg.in_offpeak_window(15));
    }

    #[test]
    fn prefixed_key_wins_over_bare() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("BIND_ADDR", "0.0.0.0:1");
            jail.set_env("MEDI_BIND_ADDR", "0.0.0.0:2");
            let cfg = AppConfig::from_env().expect("extract");
            assert_eq!(cfg.bind_addr, "0.0.0.0:2");
            Ok(())
        });
    }
}
