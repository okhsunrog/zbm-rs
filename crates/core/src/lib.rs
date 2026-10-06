//! Boot-domain state. Discovery never imports, mounts or modifies a pool.
pub mod boot;
pub mod boot_zfs;
use serde::Serialize;
use std::time::Duration;
pub use zfskit::Zfs;
use zfskit::pool::DiscoveredPool;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Pool {
    pub name: String,
    pub guid: u64,
    pub health: String,
}

impl TryFrom<DiscoveredPool> for Pool {
    type Error = zfskit::ZfsError;
    fn try_from(pool: DiscoveredPool) -> Result<Self, Self::Error> {
        Ok(Self {
            guid: pool.guid()?,
            name: pool.name,
            health: pool.state,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct State {
    pub pools: Vec<Pool>,
    pub selected: usize,
    pub scanning: bool,
    pub error: Option<String>,
    pub scans: u64,
    pub targets: Vec<boot::BootTarget>,
    pub selected_target: usize,
}

impl State {
    pub fn apply_scan(&mut self, result: Result<Vec<Pool>, String>) {
        self.targets.clear();
        self.selected_target = 0;
        self.scanning = false;
        self.scans += 1;
        match result {
            Ok(pools) => {
                self.pools = pools;
                self.error = None;
            }
            Err(error) => {
                self.pools.clear();
                self.error = Some(error);
            }
        }
        self.selected = self.selected.min(self.pools.len().saturating_sub(1));
    }

    pub fn move_selection(&mut self, down: bool) {
        if !self.pools.is_empty() {
            self.selected = if down {
                (self.selected + 1) % self.pools.len()
            } else {
                (self.selected + self.pools.len() - 1) % self.pools.len()
            };
        }
    }
}

pub async fn discover(zfs: &Zfs) -> Result<Vec<Pool>, String> {
    let pools = tokio::time::timeout(Duration::from_secs(10), zfs.discover_importable_pools())
        .await
        .map_err(|_| "Pool discovery timed out after 10 seconds".to_owned())?
        .map_err(|e| e.to_string())?;
    let mut pools: Vec<Pool> = pools
        .into_iter()
        .map(Pool::try_from)
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    for pool in boot_zfs::managed_pools(zfs)
        .await
        .map_err(|e| e.to_string())?
    {
        if !pools.iter().any(|p| p.guid == pool.guid) {
            pools.push(pool);
        }
    }
    Ok(pools)
}

pub fn preview() -> Vec<Pool> {
    vec![Pool {
        name: "preview-tank".into(),
        guid: 42,
        health: "ONLINE".into(),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use zfskit::{Cmd, RecordingRunner};

    #[tokio::test]
    async fn discovery_uses_only_non_mutating_zpool_import() {
        let runner = RecordingRunner::new().record(
            Cmd::new("zpool").arg("import"),
            b"   pool: tank\n     id: 42\n  state: ONLINE\n".to_vec(),
            vec![],
            0,
        );
        assert_eq!(
            discover(&Zfs::with_runner(runner)).await.unwrap(),
            vec![Pool {
                name: "tank".into(),
                guid: 42,
                health: "ONLINE".into()
            }]
        );
    }

    #[tokio::test]
    async fn command_failures_are_not_empty_pool_success() {
        let runner = RecordingRunner::new().record(
            Cmd::new("zpool").arg("import"),
            vec![],
            b"cannot open /dev/zfs: No such file or directory".to_vec(),
            1,
        );
        assert!(discover(&Zfs::with_runner(runner)).await.is_err());
    }

    #[test]
    fn stale_selection_is_clamped_and_stale_pools_are_cleared() {
        let mut state = State {
            pools: preview(),
            selected: 9,
            ..State::default()
        };
        state.apply_scan(Err("device disappeared".into()));
        assert_eq!(state.selected, 0);
        assert!(state.pools.is_empty());
        assert_eq!(state.scans, 1);
    }
}
