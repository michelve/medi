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
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            media_dir: PathBuf::from("/media"),
            config_dir: PathBuf::from("/config"),
            bind_addr: "0.0.0.0:8096".to_string(),
            db_pool_size: 8,
            offpeak_start_hour: 2,
            offpeak_end_hour: 6,
            asset_max_concurrency: 1,
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
    /// | `BIND_ADDR`          | `bind_addr`          | `0.0.0.0:8096`   |
    /// | `DB_POOL_SIZE`       | `db_pool_size`       | `8`              |
    /// | `OFFPEAK_START_HOUR` | `offpeak_start_hour` | `2`              |
    /// | `OFFPEAK_END_HOUR`   | `offpeak_end_hour`   | `6`              |
    /// | `ASSET_MAX_CONCURRENCY` | `asset_max_concurrency` | `1`        |
    ///
    /// A `MEDI_`-prefixed form of each key is also accepted (e.g. `MEDI_BIND_ADDR`)
    /// so the vars can be namespaced when the container shares an environment.
    pub fn from_env() -> Result<Self, Error> {
        // Only our own keys — restrict the raw provider so unrelated process env
        // (PATH, HOME, …) never leaks into extraction.
        const KEYS: &[&str] = &[
            "media_dir",
            "config_dir",
            "bind_addr",
            "db_pool_size",
            "offpeak_start_hour",
            "offpeak_end_hour",
            "asset_max_concurrency",
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
            assert_eq!(cfg.bind_addr, "0.0.0.0:8096");
            assert_eq!(cfg.db_pool_size, 8);
            assert_eq!(cfg.asset_max_concurrency, 1);
            Ok(())
        });
    }

    #[test]
    fn env_overrides_each_field() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("MEDIA_DIR", "/mnt/library");
            jail.set_env("CONFIG_DIR", "/var/medi");
            jail.set_env("BIND_ADDR", "127.0.0.1:9000");
            jail.set_env("DB_POOL_SIZE", "16");
            jail.set_env("OFFPEAK_START_HOUR", "1");
            jail.set_env("OFFPEAK_END_HOUR", "7");
            jail.set_env("ASSET_MAX_CONCURRENCY", "3");

            let cfg = AppConfig::from_env().expect("env extract");
            assert_eq!(cfg.media_dir, PathBuf::from("/mnt/library"));
            assert_eq!(cfg.config_dir, PathBuf::from("/var/medi"));
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
