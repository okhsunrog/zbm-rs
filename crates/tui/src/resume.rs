//! In-process coordinator shared by the privileged broker and PID 1.
//! Linux image access lives in core; the unprivileged UI only uses broker RPC.
use anyhow::{Result, ensure};
use zbm_core::{
    boot::BootPlan,
    resume::{Status, linux::Swap},
};
const CANCELLING: &str = "/run/zbm-rs/resume-cancelling";

pub fn status(config: &crate::config::Config) -> Result<Status> {
    if config.resume.enabled() && std::path::Path::new(CANCELLING).try_exists()? {
        return Ok(Status::Blocked { reason: "Image cancellation was interrupted; reboot or power off before further disk operations".into() });
    }
    match config.resume.device() {
        None => Ok(Status::Inactive),
        Some(device) => {
            if !zbm_core::resume::mapping::available(&device)? {
                let (pool_guid, encryption_root) = config
                    .resume
                    .credential
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Missing resume credential source"))?
                    .target();
                return Ok(Status::Locked {
                    pool_guid,
                    encryption_root: encryption_root.into(),
                });
            }
            Ok(config
                .resume
                .apply_role(Swap::open_device(&device, false)?.status()?))
        }
    }
}
pub fn current() -> Result<Status> {
    status(&crate::config::load(true)?)
}
pub async fn display_status(config: &crate::config::Config) -> Status {
    if !config.resume.enabled() {
        return Status::Inactive;
    }
    let result = if crate::broker::enabled() {
        crate::broker::resume_status().await
    } else {
        tokio::task::spawn_blocking(current)
            .await
            .unwrap_or_else(|e| Err(e.into()))
    };
    result.unwrap_or_else(|e| Status::Blocked {
        reason: format!("Cannot check saved session: {e}"),
    })
}
pub fn cancel(expected: &str) -> Result<()> {
    let config = crate::config::load(true)?;
    let device = config
        .resume
        .device()
        .ok_or_else(|| anyhow::anyhow!("Resume is not configured"))?;
    // Revalidate ownership as well as the crypt identity before a writer opens.
    ensure!(
        zbm_core::resume::mapping::available(&device)?,
        "Resume device is closed"
    );
    let swap = Swap::open_device(&device, true)?;
    ensure!(
        swap.status()?.session_id() == Some(expected),
        "Saved session changed; confirmation expired"
    );
    // Keep an interrupted/failed write blocked across manager restarts, even if
    // the page cache already shows a signature not yet durably flushed.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(CANCELLING)?;
    swap.cancel(expected)?;
    ensure!(
        !swap.status()?.protected(),
        "Saved image cancellation is incomplete"
    );
    std::fs::remove_file(CANCELLING)?;
    Ok(())
}
pub fn handoff(pool_guid: u64, plan: &BootPlan) -> Result<Option<String>> {
    let config = crate::config::load(true)?;
    let Some(device) = config.resume.device() else {
        return Ok(None);
    };
    ensure!(
        zbm_core::resume::mapping::available(&device)?,
        "Resume device is closed"
    );
    let swap = Swap::open_device(&device, false)?;
    let status = swap.status()?;
    if !status.protected() {
        return Ok(None);
    }
    ensure!(
        config.resume.role != zbm_core::resume::Role::CancelOnly,
        "Rescue cannot resume; discard the saved session before booting"
    );
    swap.verify_handoff(pool_guid, plan)?;
    Ok(status.session_id().map(str::to_owned))
}
pub fn check_handoff(expected: Option<&str>) -> Result<()> {
    let status = current()?;
    ensure!(
        !matches!(status, Status::Blocked { .. } | Status::Locked { .. }),
        "Resume status is unknown"
    );
    ensure!(
        status.session_id() == expected,
        "Saved image changed after kernel preparation"
    );
    Ok(())
}

pub fn release_mapping() -> Result<()> {
    if let Some(device) = crate::config::load(true)?.resume.device() {
        zbm_core::resume::mapping::release(&device)?;
    }
    Ok(())
}

pub async fn unlock(
    zfs: &zbm_core::Zfs,
    pool: &zbm_core::Pool,
    dataset: &str,
    answer: Option<zbm_core::unlock::Answer>,
) -> Result<zbm_core::unlock::Resolution> {
    let config = crate::config::load(true)?;
    if let Some(source) = &config.resume.credential
        && source.matches(pool.guid, dataset)
    {
        return Ok(zbm_core::unlock::resolve_resume(
            zfs,
            pool,
            source,
            &config.resume.device().unwrap(),
            answer,
        )
        .await?);
    }
    Ok(zbm_core::unlock::resolve(zfs, pool, dataset, answer).await?)
}

/// Native early-boot and pre-hibernation roles of the same executable.
pub fn utility(args: &[std::ffi::OsString]) -> Result<bool> {
    if args
        .first()
        .is_some_and(|arg| arg == "--resume-capabilities")
    {
        anyhow::ensure!(args.len() == 1, "usage: zbm-rs --resume-capabilities");
        println!(
            "{}",
            serde_json::json!({
                "version": 1,
                "record_protocol": zbm_core::resume::linux::RECORD_PROTOCOL,
                "durable_attempt_tracking": true,
                "cancel_only": true,
            })
        );
        return Ok(true);
    }
    if args
        .first()
        .is_some_and(|arg| arg == "--resume-status-config")
    {
        anyhow::ensure!(
            unsafe { libc::geteuid() } == 0 && args.len() == 2,
            "usage (root): zbm-rs --resume-status-config IMAGE_CONFIG"
        );
        let config = crate::config::parse(&std::fs::read_to_string(&args[1])?)?;
        let device = config
            .resume
            .device()
            .ok_or_else(|| anyhow::anyhow!("Resume is not configured"))?;
        let status = config
            .resume
            .apply_role(Swap::open_device(&device, false)?.status()?);
        println!("{}", serde_json::to_string(&status)?);
        return Ok(true);
    }
    if args.first().is_some_and(|arg| {
        arg == "--resume-record-current" || arg == "--resume-record-current-config"
    }) {
        let configured = args[0] == "--resume-record-current-config";
        anyhow::ensure!(
            unsafe { libc::geteuid() } == 0 && args.len() == if configured { 3 } else { 4 },
            "usage (root): zbm-rs --resume-record-current-config DATASET IMAGE_CONFIG"
        );
        let dataset_index = if configured { 1 } else { 2 };
        let dataset = args[dataset_index]
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid dataset"))?;
        let config = crate::config::parse(&std::fs::read_to_string(&args[dataset_index + 1])?)?;
        anyhow::ensure!(
            config.resume.role != zbm_core::resume::Role::CancelOnly,
            "Cancellation-only rescue cannot record a hibernation image"
        );
        let device = if configured {
            config
                .resume
                .device()
                .ok_or_else(|| anyhow::anyhow!("Resume is not configured"))?
        } else {
            zbm_core::resume::device::Device::Partition {
                partuuid: args[1]
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("Invalid PARTUUID"))?
                    .into(),
            }
        };
        let root = std::process::Command::new("findmnt")
            .args([
                "--noheadings",
                "--raw",
                "--output",
                "SOURCE,FSTYPE",
                "--target",
                "/",
            ])
            .output()?;
        anyhow::ensure!(
            root.status.success(),
            "Cannot identify the running root filesystem"
        );
        let root = std::str::from_utf8(&root.stdout)?
            .split_whitespace()
            .collect::<Vec<_>>();
        anyhow::ensure!(
            root == [dataset, "zfs"],
            "Running root differs from the configured resume dataset"
        );
        let booted = std::fs::canonicalize("/run/booted-system")?;
        let targets = zbm_core::boot::generations(std::path::Path::new("/"), dataset, 512)?;
        let target = targets
            .into_iter()
            .find(|t| t.toplevel() == Some(booted.as_path()))
            .ok_or_else(|| {
                anyhow::anyhow!("Booted generation is not available in the published catalog")
            })?;
        let plan = zbm_core::boot::BootPlan::resolve(&target, &config.kernel_args)?;
        zbm_core::resume::linux::verify_running_cmdline(&plan.cmdline)?;
        let output = std::process::Command::new("zpool")
            .args([
                "get",
                "-H",
                "-o",
                "value",
                "guid",
                dataset.split('/').next().unwrap(),
            ])
            .output()?;
        anyhow::ensure!(output.status.success(), "Cannot read the running pool GUID");
        let guid = std::str::from_utf8(&output.stdout)?.trim().parse()?;
        zbm_core::resume::linux::Swap::open_device(&device, true)?.record(guid, &plan)?;
        return Ok(true);
    }
    if args.first().is_some_and(|arg| {
        arg == "--resume-record"
            || arg == "--resume-record-config"
            || arg == "--resume-restore"
            || arg == "--resume-restore-config"
            || arg == "--resume-finish-config"
    }) {
        anyhow::ensure!(
            unsafe { libc::geteuid() } == 0,
            "Resume device operations require root"
        );
        let recording = args[0] == "--resume-record" || args[0] == "--resume-record-config";
        anyhow::ensure!(
            args.len() == if recording { 4 } else { 2 },
            "usage: zbm-rs --resume-record PARTUUID POOL_GUID PLAN_JSON | --resume-restore PARTUUID"
        );
        let finishing = args[0] == "--resume-finish-config";
        let device = if args[0] == "--resume-restore-config"
            || args[0] == "--resume-record-config"
            || finishing
        {
            let config = crate::config::parse(&std::fs::read_to_string(&args[1])?)?;
            anyhow::ensure!(
                config.resume.role != zbm_core::resume::Role::CancelOnly,
                "Cancellation-only rescue cannot record or restore a hibernation image"
            );
            config
                .resume
                .device()
                .ok_or_else(|| anyhow::anyhow!("Resume is not configured"))?
        } else {
            zbm_core::resume::device::Device::Partition {
                partuuid: args[1]
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("Invalid PARTUUID"))?
                    .into(),
            }
        };
        let swap = zbm_core::resume::linux::Swap::open_device(&device, recording || finishing)?;
        if finishing {
            swap.finish_attempt()?;
        } else if recording {
            use std::io::Read;
            let guid: u64 = args[2]
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid pool GUID"))?
                .parse()?;
            let mut bytes = Vec::new();
            std::fs::File::open(&args[3])?
                .take(65537)
                .read_to_end(&mut bytes)?;
            anyhow::ensure!(bytes.len() <= 65536, "Boot plan exceeds size limit");
            let plan = serde_json::from_slice(&bytes)?;
            swap.record(guid, &plan)?;
        } else {
            if let Status::Pending { session } = swap.status()? {
                let id = session.id;
                drop(swap);
                let writer = Swap::open_device(&device, true)?;
                writer.arm_attempt(&id)?;
                drop(writer);
                // The kernel needs to open this device itself. No exclusive
                // userspace writer may survive across the resume syscall.
                let swap = Swap::open_device(&device, false)?;
                swap.restore()?;
                return Ok(true);
            }
            swap.restore()?;
        }
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_support_does_not_access_devices() {
        assert!(matches!(
            status(&crate::config::Config::default()).unwrap(),
            Status::Inactive
        ));
    }
}
