//! Periodic SQLite backups with retention (production durability).

use std::path::{Path, PathBuf};
use std::time::Duration;
use tailsvc_storage::Storage;
use tracing::{error, info, warn};

#[derive(Debug, Clone)]
pub struct BackupConfig {
    pub dir: PathBuf,
    pub interval: Duration,
    pub keep: usize,
}

/// Run forever: snapshot DB on an interval, prune old files.
pub async fn run_backup_loop(storage: Storage, cfg: BackupConfig) {
    if cfg.keep == 0 {
        warn!("backup.keep=0 disables retention pruning only; backups still written");
    }
    if let Err(e) = std::fs::create_dir_all(&cfg.dir) {
        error!(error = %e, dir = %cfg.dir.display(), "cannot create backup dir; backup loop idle");
        // Do not panic — sit idle so process stays up; ops can fix disk and restart.
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }

    let mut interval = tokio::time::interval(cfg.interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        match storage.backup_to_dir(&cfg.dir).await {
            Ok(path) => {
                info!(path = %path.display(), "sqlite backup ok");
                if cfg.keep > 0 {
                    if let Err(e) = prune_old_backups(&cfg.dir, cfg.keep) {
                        warn!(error = %e, "backup prune failed");
                    }
                }
            }
            Err(e) => {
                error!(error = %e, "sqlite backup failed");
            }
        }
    }
}

fn prune_old_backups(dir: &Path, keep: usize) -> std::io::Result<()> {
    let mut files: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("controller-") && n.ends_with(".db"))
                .unwrap_or(false)
        })
        .collect();
    files.sort_by_key(|e| std::cmp::Reverse(e.path()));
    for old in files.into_iter().skip(keep) {
        let p = old.path();
        if let Err(e) = std::fs::remove_file(&p) {
            warn!(error = %e, path = %p.display(), "remove old backup");
        }
    }
    Ok(())
}
