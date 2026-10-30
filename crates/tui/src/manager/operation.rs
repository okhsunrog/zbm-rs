//! One owned boot operation at a time. Dropping it cancels its async commands;
//! resource intent remains in core's ledger for the next manager to reconcile.
use super::executor::LoadedKernel;
use anyhow::Result;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use zbm_core::{
    Pool, Zfs,
    activity::{Activity, RecoveryAction},
    boot::{BootPlan, BootTarget, SnapshotSource, TargetDiscovery},
    boot_zfs::{self, Snapshot},
    environment::BootEnvironment,
};

const DEADLINE: Duration = Duration::from_secs(30);

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Action {
    CancelResume {
        session_id: String,
    },
    Unlock {
        pool: Pool,
        dataset: String,
        answer: Option<zbm_core::unlock::Answer>,
        readonly: bool,
        limit: u32,
    },
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
    ColdBoot {
        pool: Pool,
        target: BootTarget,
        args: Vec<String>,
    },
}

pub enum Outcome {
    ResumeCancelled,
    Unlock(zbm_core::unlock::Status),
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
    fn recovery(&self) -> RecoveryAction {
        match self {
            Self::CancelResume { .. } => RecoveryAction::RestartRequired,
            Self::Rollback { .. } | Self::Clone { .. } => RecoveryAction::RestartRequired,
            _ => RecoveryAction::Retry,
        }
    }
    fn description(&self) -> &'static str {
        match self {
            Self::CancelResume { .. } => "Cancelling saved session",
            Self::Unlock { .. } => "Unlocking selected boot environment",
            Self::Rollback { .. } => "Rolling back selected dataset",
            Self::Clone { .. } => "Creating persistent boot environment",
            Self::Environments { .. } => "Inspecting selected pool",
            Self::Snapshots { .. } => "Listing snapshots",
            Self::Targets { .. } => "Inspecting snapshot boot targets",
            Self::Prepare { .. } => "Preparing snapshot clone",
            Self::Discard { .. } => "Discarding prepared clone",
            Self::Boot { .. } | Self::ColdBoot { .. } => "Preparing kernel handoff",
        }
    }

    fn error_event(&self) -> &'static str {
        match self {
            Self::CancelResume { .. } => "resume-cancel-error",
            Self::Unlock { .. } => "unlock-error",
            Self::Rollback { .. } => "rollback-error",
            Self::Clone { .. } => "clone-error",
            Self::Environments { .. } => "boot-environments",
            Self::Snapshots { .. } => "snapshots",
            Self::Targets { .. } => "boot-targets",
            Self::Prepare { .. } | Self::Discard { .. } => "snapshot-error",
            Self::Boot { .. } | Self::ColdBoot { .. } => "boot-error",
        }
    }

    async fn run(self) -> Result<Outcome> {
        if crate::broker::enabled() {
            return crate::broker::perform(self).await;
        }
        self.run_local().await
    }

    pub(crate) async fn run_local(mut self) -> Result<Outcome> {
        let zfs = Zfs::new();
        if let Self::CancelResume { session_id } = &self {
            let id = session_id.clone();
            tokio::task::spawn_blocking(move || crate::resume::cancel(&id)).await??;
            return Ok(Outcome::ResumeCancelled);
        }
        let resume = crate::resume::current()?;
        self.guard_resume(&resume)?;
        let cold_boot = matches!(self, Self::ColdBoot { .. });
        if let Self::Environments { readonly, .. } = &mut self {
            *readonly |= resume.protected();
        }
        if !resume.protected()
            && let Some(pool) = self.write_pool()
        {
            let config = crate::config::load(true)?;
            if config.resume.enabled()
                && !matches!(
                    config.zfs.import_policy,
                    crate::config::ImportPolicy::ReadOnly
                )
            {
                // A cancelled session may leave a read-only catalog import.
                boot_zfs::reopen_writable(&zfs, pool, config.nixos.generation_limit).await?;
            }
        }
        if crate::config::image_enforced()
            && let Self::Prepare { target, args, .. }
            | Self::Boot { target, args, .. }
            | Self::ColdBoot { target, args, .. } = &self
        {
            let target = target.clone();
            let args = args.clone();
            let config = crate::config::load(true)?;
            // Authorize immutable snapshot inputs before any persistent clone
            // intent or ZFS mutation. Handoff repeats this against the final clone.
            tokio::task::spawn_blocking(move || {
                let plan = if target.snapshot.is_some() {
                    BootPlan::for_snapshot(&target, &target.dataset, &args)?
                } else {
                    BootPlan::resolve_verified(&target, &args)?
                };
                let authorization = zbm_core::security::Authorization::load_for_plan(
                    &plan,
                    &config.security.target_authorities,
                )?;
                authorization.policy().matches_selection(&plan)
            })
            .await??;
        }
        Ok(match self {
            Self::CancelResume { .. } => unreachable!(),
            Self::Unlock {
                pool,
                dataset,
                answer,
                ..
            } => Outcome::Unlock(crate::resume::unlock(&zfs, &pool, &dataset, answer).await?),
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
            } => Outcome::Environments(
                boot_zfs::environment_catalog(
                    &zfs,
                    &pool,
                    readonly,
                    limit,
                    !matches!(resume, zbm_core::resume::Status::Locked { .. }),
                )
                .await?,
            ),
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
            Self::Boot { pool, target, args } | Self::ColdBoot { pool, target, args } => {
                let target = boot_zfs::refresh_target(&zfs, &target).await?;
                let plan = if target.snapshot.is_some() {
                    boot_zfs::prepare_snapshot(&zfs, &pool, &target, &args).await?
                } else {
                    tokio::task::spawn_blocking(move || {
                        if crate::config::image_enforced() {
                            BootPlan::resolve_verified(&target, &args)
                        } else {
                            BootPlan::resolve(&target, &args)
                        }
                    })
                    .await??
                };
                let load_plan = plan.clone();
                let owned_clone = boot_zfs::validate_handoff_clone(&zfs, &pool, &plan).await?;
                let mut loaded = tokio::task::spawn_blocking(move || {
                    if crate::config::image_enforced() {
                        LoadedKernel::load_verified(&load_plan, owned_clone.as_ref())
                    } else {
                        LoadedKernel::load(&load_plan)
                    }
                })
                .await??;
                if cold_boot {
                    // A newly appearing image must never turn an explicit cold
                    // boot into resume. LoadedKernel repeats this at execution.
                    crate::resume::check_handoff(None)?;
                } else {
                    loaded.bind_resume(pool.guid, &plan)?;
                }
                boot_zfs::retain_snapshot_clone(&zfs, &pool, &plan).await?;
                boot_zfs::release_owned(&zfs, &pool).await?;
                Outcome::Boot(plan, loaded)
            }
        })
    }

    pub(crate) fn guard_resume(&self, status: &zbm_core::resume::Status) -> Result<()> {
        use zbm_core::resume::Status;
        if matches!(self, Self::CancelResume { .. }) {
            return Ok(());
        }
        if let Status::Blocked { reason } = status {
            anyhow::bail!("Resume status unavailable: {reason}");
        }
        if !status.protected() {
            return Ok(());
        }
        if let Status::Locked {
            pool_guid,
            encryption_root,
        } = status
        {
            return match self {
                Self::Environments { .. } => Ok(()),
                Self::Unlock { pool, dataset, .. }
                    if pool.guid == *pool_guid && dataset == encryption_root =>
                {
                    Ok(())
                }
                _ => anyhow::bail!(
                    "Unlock the configured encrypted swap before further boot actions"
                ),
            };
        }
        match self {
            Self::Unlock { .. }
            | Self::Environments { .. }
            | Self::Snapshots { .. }
            | Self::Targets { .. } => Ok(()),
            Self::Boot { pool, target, .. } if status.matches(pool.guid, target) => Ok(()),
            _ => anyhow::bail!("Cancel the saved session before this action"),
        }
    }

    fn write_pool(&self) -> Option<&Pool> {
        match self {
            Self::Prepare { pool, .. }
            | Self::Discard { pool, .. }
            | Self::Clone { pool, .. }
            | Self::Rollback { pool, .. } => Some(pool),
            Self::Boot { pool, target, .. } | Self::ColdBoot { pool, target, .. }
                if target.snapshot.is_some() =>
            {
                Some(pool)
            }
            _ => None,
        }
    }
}

pub struct Pending {
    pub description: &'static str,
    pub error_event: &'static str,
    task: JoinHandle<Result<Outcome>>,
    deadline: Instant,
    recovery: RecoveryAction,
}

#[derive(Debug)]
pub struct OperationFailure {
    pub source: anyhow::Error,
    pub recovery: RecoveryAction,
}

impl OperationFailure {
    fn new(source: anyhow::Error, recovery: RecoveryAction) -> Self {
        let recovery = if source.is::<tokio::task::JoinError>() {
            RecoveryAction::RestartRequired
        } else {
            recovery
        };
        Self { source, recovery }
    }
}

impl Pending {
    pub fn start(action: Action) -> Self {
        Self {
            description: action.description(),
            error_event: action.error_event(),
            recovery: action.recovery(),
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

    pub async fn finish(&mut self) -> std::result::Result<Outcome, OperationFailure> {
        (&mut self.task)
            .await
            .map_err(anyhow::Error::from)
            .and_then(std::convert::identity)
            .map_err(|error| OperationFailure::new(error, self.recovery))
    }
}

/// Owns the pending handle and phase together. UI state is only a projection.
#[derive(Default)]
pub struct Workflow {
    phase: Activity<Pending>,
}

impl Workflow {
    pub fn view(&self) -> Activity {
        self.phase.project(|pending| {
            format!(
                "{}. Restart and Power off remain available.",
                pending.description
            )
        })
    }
    pub fn start_scan(&mut self) -> bool {
        if !self.phase.is_idle() {
            return false;
        }
        self.phase = Activity::Scanning;
        true
    }
    pub fn finish_scan(&mut self) {
        if self.phase.is_scanning() {
            self.phase = Activity::Idle;
        }
    }
    pub fn start(&mut self, action: Action) -> Result<()> {
        anyhow::ensure!(
            self.phase.is_idle(),
            "An operation cannot start in the current phase"
        );
        self.phase = Activity::Running(Pending::start(action));
        Ok(())
    }
    pub fn take_finished(&mut self) -> Option<Pending> {
        if !self.phase.operation().is_some_and(Pending::is_finished) {
            return None;
        }
        match std::mem::take(&mut self.phase) {
            Activity::Running(pending) => Some(pending),
            _ => unreachable!("finished operation is running"),
        }
    }
    pub fn expire(&mut self) -> bool {
        if !self.phase.operation().is_some_and(Pending::expired) {
            return false;
        }
        // Dropping the handle aborts async work; persistent intent stays journaled.
        self.phase = Activity::RestartRequired;
        true
    }
    pub fn recover(&mut self, action: RecoveryAction) {
        self.phase = Activity::recovered(action);
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action() -> Action {
        Action::Snapshots {
            pool: Pool {
                name: "fixture".into(),
                guid: 1,
                health: "ONLINE".into(),
            },
            dataset: "fixture/root".into(),
        }
    }

    #[test]
    fn closed_swap_allows_only_metadata_and_its_bound_unlock() {
        let pool = Pool {
            name: "tank".into(),
            guid: 42,
            health: "ONLINE".into(),
        };
        let status = zbm_core::resume::Status::Locked {
            pool_guid: 42,
            encryption_root: "tank/root".into(),
        };
        assert!(status.protected());
        assert!(
            Action::Environments {
                pool: pool.clone(),
                readonly: false,
                limit: 1
            }
            .guard_resume(&status)
            .is_ok()
        );
        for (guid, dataset, allowed) in [
            (42, "tank/root", true),
            (43, "tank/root", false),
            (42, "tank/other", false),
        ] {
            let mut selected = pool.clone();
            selected.guid = guid;
            assert_eq!(
                Action::Unlock {
                    pool: selected,
                    dataset: dataset.into(),
                    answer: None,
                    readonly: false,
                    limit: 1
                }
                .guard_resume(&status)
                .is_ok(),
                allowed
            );
        }
        assert!(action().guard_resume(&status).is_err());
    }

    #[test]
    fn cold_boot_rejects_even_a_matching_saved_session() {
        let target: BootTarget = serde_json::from_value(serde_json::json!({
            "dataset":"tank/root","generation":7,"label":"NixOS","root":"/guest",
            "toplevel":"/nix/store/base","backend":{"kind":"Nixos"},"snapshot":null,
            "mountpoint":null,"issue":null,
            "inputs":{"kernel":"/shared/kernel","initrd":null,"init":"/nix/store/base/init","kernel_params":[]}
        })).unwrap();
        let pool = Pool {
            name: "tank".into(),
            guid: 42,
            health: "ONLINE".into(),
        };
        let status = zbm_core::resume::Status::Pending {
            session: Box::new(zbm_core::resume::Session {
                id: "new-image".into(),
                pool_guid: 42,
                target: target.clone(),
            }),
        };
        let ordinary = Action::Boot {
            pool: pool.clone(),
            target: target.clone(),
            args: vec![],
        };
        let cold = Action::ColdBoot {
            pool,
            target,
            args: vec![],
        };
        assert!(ordinary.guard_resume(&status).is_ok());
        assert!(cold.guard_resume(&status).is_err());
        assert!(
            cold.guard_resume(&zbm_core::resume::Status::Inactive)
                .is_ok()
        );
    }

    #[test]
    fn scan_and_restart_phases_reject_concurrent_operations() {
        let mut workflow = Workflow::default();
        assert!(workflow.start_scan());
        assert!(!workflow.start_scan());
        assert!(workflow.start(action()).is_err());
        workflow.finish_scan();
        assert!(workflow.view().is_idle());
        workflow.recover(RecoveryAction::RestartRequired);
        assert!(!workflow.start_scan());
        assert!(workflow.start(action()).is_err());
        workflow.finish_scan();
        assert!(workflow.view().requires_restart());
    }

    #[tokio::test]
    async fn expiration_aborts_work_and_requires_reconciliation() {
        let task = tokio::spawn(std::future::pending::<Result<Outcome>>());
        let abort = task.abort_handle();
        let mut workflow = Workflow {
            phase: Activity::Running(Pending {
                description: "fixture",
                error_event: "irrelevant-name",
                task,
                deadline: Instant::now() - Duration::from_secs(1),
                recovery: RecoveryAction::Retry,
            }),
        };
        assert!(workflow.expire());
        tokio::task::yield_now().await;
        assert!(abort.is_finished());
        assert!(workflow.view().requires_restart());
        assert!(workflow.take_finished().is_none());
        assert!(!workflow.expire());
    }

    #[tokio::test]
    async fn recovery_is_typed_and_worker_failures_require_restart() {
        let failure =
            OperationFailure::new(anyhow::anyhow!("ordinary error"), RecoveryAction::Retry);
        assert_eq!(failure.recovery, RecoveryAction::Retry);
        let failure = OperationFailure::new(
            anyhow::anyhow!("partial mutation"),
            RecoveryAction::RestartRequired,
        );
        assert_eq!(failure.recovery, RecoveryAction::RestartRequired);
        let task = tokio::spawn(std::future::pending::<()>());
        task.abort();
        let failure = OperationFailure::new(task.await.unwrap_err().into(), RecoveryAction::Retry);
        assert_eq!(failure.recovery, RecoveryAction::RestartRequired);
        let pool = Pool {
            name: "fixture".into(),
            guid: 1,
            health: "ONLINE".into(),
        };
        assert_eq!(
            Action::Rollback {
                pool,
                source: SnapshotSource {
                    name: "fixture/root@s".into(),
                    guid: 1
                },
                limit: 1
            }
            .recovery(),
            RecoveryAction::RestartRequired
        );
        assert_eq!(action().recovery(), RecoveryAction::Retry);
    }
}
