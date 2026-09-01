//! Artwork lifecycle reaping (`docs/.tasks/60` §Orphan reaping).
//!
//! Enrichment writes binary artwork into `/config/images/<kind>/<id>/`. When a title is
//! removed its directory must go too, or the cache leaks. Two entry points:
//!
//! - [`remove_title_images`] — targeted deletion of one title's directory, called on a
//!   reap or a Phase B `DELETE /api/libraries/:id` cascade.
//! - [`sweep_orphan_images`] — the opportunistic backstop: walk `images/movies` and
//!   `images/series`, delete any `<id>/` dir whose id is not among the surviving title
//!   ids. Runs off the request path (e.g. after a scan) so a crash mid-reap or a manual
//!   DB edit still converges.

use std::path::{Path, PathBuf};

use medi_db::writes::TitleKind;
use medi_db::Db;

use crate::Result;

fn kind_dir(kind: TitleKind) -> &'static str {
    match kind {
        TitleKind::Movie => "movies",
        TitleKind::Series => "series",
    }
}

/// Delete `<images_dir>/<kind>/<id>/` and everything under it. Idempotent: a missing
/// directory is a no-op (the title may never have had artwork). Runs the blocking fs
/// removal on the blocking pool.
pub async fn remove_title_images(images_dir: &Path, kind: TitleKind, id: i64) -> Result<()> {
    let dir = images_dir.join(kind_dir(kind)).join(id.to_string());
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    })
    .await??;
    Ok(())
}

/// Reconcile `<images_dir>/{movies,series}` against the ids surviving in the DB, deleting
/// any orphaned `<id>/` directory. Returns the number of directories removed. Safe to run
/// repeatedly; does nothing when the images root does not exist yet.
pub async fn sweep_orphan_images(db: &Db, images_dir: &Path) -> Result<usize> {
    // Snapshot surviving ids (one cheap read per kind).
    let db2 = db.clone();
    let (movie_ids, series_ids) = tokio::task::spawn_blocking(move || -> medi_db::DbResult<_> {
        let conn = db2.conn()?;
        let m = medi_db::queries::all_title_ids(&conn, TitleKind::Movie)?;
        let s = medi_db::queries::all_title_ids(&conn, TitleKind::Series)?;
        Ok((m, s))
    })
    .await??;

    let images_dir = images_dir.to_path_buf();
    let removed = tokio::task::spawn_blocking(move || -> std::io::Result<usize> {
        let mut removed = 0;
        removed += reap_kind(&images_dir, "movies", &movie_ids)?;
        removed += reap_kind(&images_dir, "series", &series_ids)?;
        Ok(removed)
    })
    .await??;
    if removed > 0 {
        tracing::info!(removed, "swept orphaned artwork directories");
    }
    Ok(removed)
}

/// Remove `<images_dir>/<kind_dir>/<id>` directories whose numeric id is not in
/// `surviving`. A non-numeric or unreadable entry is left alone (defensive).
fn reap_kind(images_dir: &Path, kind_dir: &str, surviving: &[i64]) -> std::io::Result<usize> {
    let root: PathBuf = images_dir.join(kind_dir);
    let entries = match std::fs::read_dir(&root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|s| s.parse::<i64>().ok()) else {
            continue; // not an id-named dir; leave it
        };
        if !surviving.contains(&id) {
            std::fs::remove_dir_all(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medi_db::writes;

    #[tokio::test]
    async fn remove_title_images_is_targeted_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let images = dir.path().join("images");
        let movie_dir = images.join("movies/7");
        std::fs::create_dir_all(&movie_dir).unwrap();
        std::fs::write(movie_dir.join("poster.jpg"), b"x").unwrap();
        // A sibling that must survive.
        std::fs::create_dir_all(images.join("movies/8")).unwrap();

        remove_title_images(&images, TitleKind::Movie, 7).await.unwrap();
        assert!(!images.join("movies/7").exists());
        assert!(images.join("movies/8").exists());

        // Second call on the now-absent dir is a no-op.
        remove_title_images(&images, TitleKind::Movie, 7).await.unwrap();
    }

    #[tokio::test]
    async fn sweep_removes_only_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        let images = dir.path().join("images");

        // Two movies survive; one is deleted.
        let (m1, m2) = {
            let conn = db.conn().unwrap();
            let a = writes::find_or_create_movie(&conn, "A", "a", None, 0).unwrap();
            let b = writes::find_or_create_movie(&conn, "B", "b", None, 0).unwrap();
            (a, b)
        };
        for id in [m1, m2, 999] {
            let d = images.join(format!("movies/{id}"));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("poster.jpg"), b"x").unwrap();
        }
        // A non-numeric dir must be left alone.
        std::fs::create_dir_all(images.join("movies/tmp-junk")).unwrap();

        let removed = sweep_orphan_images(&db, &images).await.unwrap();
        assert_eq!(removed, 1, "only the 999 orphan is reaped");
        assert!(images.join(format!("movies/{m1}")).exists());
        assert!(images.join(format!("movies/{m2}")).exists());
        assert!(!images.join("movies/999").exists());
        assert!(images.join("movies/tmp-junk").exists());
    }

    #[tokio::test]
    async fn sweep_no_images_dir_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let db = medi_db::open(dir.path().join("library.db"), 2).unwrap();
        let removed = sweep_orphan_images(&db, &dir.path().join("nope")).await.unwrap();
        assert_eq!(removed, 0);
    }
}
