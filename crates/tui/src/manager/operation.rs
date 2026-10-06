//! One owned boot operation at a time. Dropping it cancels its async commands;
//! resource intent remains in core's ledger for the next manager to reconcile.
use super::executor::LoadedKernel;
use anyhow::Result;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use zbm_core::{
    Pool, Zfs,
    boot::{BootPlan, BootTarget, SnapshotSource, TargetDiscovery},
    boot_zfs::{self, Snapshot},
    environment::BootEnvironment,
};

const DEADLINE: Duration = Duration::from_secs(30);

pub enum Action {
    Rollback {
        pool: Pool,
        source: SnapshotSource,
        limit: u32,
    },
    Clone {
        pool: Pool,
        source: SnapshotSource,
        promote: bool,
        limit: u32,
    },
    Environments {
        pool: Pool,
        readonly: bool,
        limit: u32,
    },
    Snapshots {
        pool: Pool,
        dataset: String,
    },
    Targets {
        pool: Pool,
        snapshot: Snapshot,
        limit: u32,
    },
    Prepare {
        pool: Pool,
        target: BootTarget,
        args: Vec<String>,
    },
    Discard {
        pool: Pool,
        source: SnapshotSource,
    },
    Boot {
        pool: Pool,
        target: BootTarget,
        args: Vec<String>,
    },
}

pub enum Outcome {
    Changed {
        environments: Vec<BootEnvironment>,
        selected: String,
        message: String,
    },
    Environments(Vec<BootEnvironment>),
    Snapshots(Vec<Snapshot>),
    Targets(TargetDiscovery),
    Prepared(BootPlan),
    Discarded(String),
    Boot(BootPlan, LoadedKernel),
}

impl Action {
    fn description(&self) -> &'static str {
        match self {
            Self::Rollback { .. } => "Rolling back selected dataset",
            Self::Clone { .. } => "Creating persistent boot environment",
            Self::Environments { .. } => "Inspecting selected pool",
            Self::Snapshots { .. } => "Listing snapshots",
            Self::Targets { .. } => "Inspecting snapshot boot targets",
            Self::Prepare { .. } => "Preparing snapshot clone",
            Self::Discard { .. } => "Discarding prepared clone",
            Self::Boot { .. } => "Preparing kernel handoff",
        }
    }

    fn error_event(&self) -> &'static str {
        match self {
            Self::Rollback { .. } => "rollback-error",
            Self::Clone { .. } => "clone-error",
            Self::Environments { .. } => "boot-environments",
            Self::Snapshots { .. } => "snapshots",
            Self::Targets { .. } => "boot-targets",
            Self::Prepare { .. } | Self::Discard { .. } => "snapshot-error",
            Self::Boot { .. } => "boot-error",
        }
    }

    async fn run(self) -> Result<Outcome> {
        let zfs = Zfs::new();
        Ok(match self {
            Self::Rollback {
                pool,
                source,
                limit,
            } => {
                boot_zfs::rollback(&zfs, &pool, &source).await?;
                Outcome::Changed {
                    environments: boot_zfs::environments(&zfs, &pool, false, limit).await?,
                    selected: source.name.split('@').next().unwrap().into(),
                    message: format!("Rolled back to {}", source.name),
                }
            }
            Self::Clone {
                pool,
                source,
                promote,
                limit,
            } => {
                let dataset = boot_zfs::clone_environment(&zfs, &pool, &source, promote).await?;
                Outcome::Changed {
                    environments: boot_zfs::environments(&zfs, &pool, false, limit).await?,
                    selected: dataset.clone(),
                    message: format!(
                        "Created {}{}",
                        dataset,
                        if promote { " (promoted)" } else { "" }
                    ),
                }
            }
            Self::Environments {
                pool,
                readonly,
                limit,
            } => Outcome::Environments(boot_zfs::environments(&zfs, &pool, readonly, limit).await?),
            Self::Snapshots { pool, dataset } => {
                Outcome::Snapshots(boot_zfs::snapshots(&zfs, &pool, &dataset).await?)
            }
            Self::Targets {
                pool,
                snapshot,
                limit,
            } => Outcome::Targets(boot_zfs::snapshot_targets(&zfs, &pool, &snapshot, limit).await?),
            Self::Prepare { pool, target, args } => {
                Outcome::Prepared(boot_zfs::prepare_snapshot(&zfs, &pool, &target, &args).await?)
            }
            Self::Discard { pool, source } => {
                Outcome::Discarded(boot_zfs::discard_prepared_clone(&zfs, &pool, &source).await?)
            }
            Self::Boot { pool, target, args } => {
                let target = boot_zfs::refresh_target(&zfs, &target).await?;
                let plan = if target.snapshot.is_some() {
                    boot_zfs::prepare_snapshot(&zfs, &pool, &target, &args).await?
                } else {
                    tokio::task::spawn_blocking(move || BootPlan::resolve(&target, &args)).await??
                };
                let load_plan = plan.clone();
                let loaded =
                    tokio::task::spawn_blocking(move || LoadedKernel::load(&load_plan)).await??;
                boot_zfs::retain_snapshot_clone(&zfs, &pool, &plan).await?;
                boot_zfs::release_owned(&zfs, &pool).await?;
                Outcome::Boot(plan, loaded)
            }
        })
    }
}

pub struct Pending {
    pub description: &'static str,
    pub error_event: &'static str,
    task: JoinHandle<Result<Outcome>>,
    deadline: Instant,
}

impl Pending {
    pub fn start(action: Action) -> Self {
        Self {
            description: action.description(),
            error_event: action.error_event(),
            task: tokio::spawn(action.run()),
            deadline: Instant::now() + DEADLINE,
        }
    }

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    pub fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    pub async fn finish(&mut self) -> Result<Outcome> {
        (&mut self.task).await?
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.task.abort();
    }
}
