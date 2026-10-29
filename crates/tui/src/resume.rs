//! In-process coordinator shared by the privileged broker and PID 1.
//! Linux image access lives in core; the unprivileged UI only uses broker RPC.
use anyhow::{Result, ensure};
use zbm_core::{
    boot::BootPlan,
    resume::{Status, linux::Swap},
};
const CANCELLING: &str = "/run/zbm-rs/resume-cancelling";

pub fn status(config: &crate::config::Config) -> Result<Status> {
    if config.resume.swap_partuuid.is_some() && std::path::Path::new(CANCELLING).try_exists()? {
        return Ok(Status::Blocked { reason: "Image cancellation was interrupted; reboot or power off before further disk operations".into() });
    }
    match &config.resume.swap_partuuid {
        None => Ok(Status::Inactive),
        Some(id) => Ok(Swap::open(id, false)?.status()?),
    }
}
pub fn current() -> Result<Status> {
    status(&crate::config::load(true)?)
}
pub async fn display_status(config: &crate::config::Config) -> Status {
    if config.resume.swap_partuuid.is_none() {
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
    let id = config
        .resume
        .swap_partuuid
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Resume is not configured"))?;
    let swap = Swap::open(id, true)?;
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
    let Some(id) = config.resume.swap_partuuid.as_ref() else {
        return Ok(None);
    };
    let swap = Swap::open(id, false)?;
    let status = swap.status()?;
    if !status.protected() {
        return Ok(None);
    }
    swap.verify_handoff(pool_guid, plan)?;
    Ok(status.session_id().map(str::to_owned))
}
pub fn check_handoff(expected: Option<&str>) -> Result<()> {
    let status = current()?;
    ensure!(
        !matches!(status, Status::Blocked { .. }),
        "Resume status is unknown"
    );
    ensure!(
        status.session_id() == expected,
        "Saved image changed after kernel preparation"
    );
    Ok(())
}

/// Native early-boot and pre-hibernation roles of the same executable.
pub fn utility(args: &[std::ffi::OsString]) -> Result<bool> {
    if args
        .first()
        .is_some_and(|arg| arg == "--resume-record-current")
    {
        anyhow::ensure!(
            unsafe { libc::geteuid() } == 0 && args.len() == 4,
            "usage (root): zbm-rs --resume-record-current PARTUUID DATASET IMAGE_CONFIG"
        );
        let id = args[1]
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid PARTUUID"))?;
        let dataset = args[2]
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid dataset"))?;
        let config = crate::config::parse(&std::fs::read_to_string(&args[3])?)?;
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
        zbm_core::resume::linux::Swap::open(id, true)?.record(guid, &plan)?;
        return Ok(true);
    }
    if args
        .first()
        .is_some_and(|arg| arg == "--resume-record" || arg == "--resume-restore")
    {
        anyhow::ensure!(
            unsafe { libc::geteuid() } == 0,
            "Resume device operations require root"
        );
        let recording = args[0] == "--resume-record";
        anyhow::ensure!(
            args.len() == if recording { 4 } else { 2 },
            "usage: zbm-rs --resume-record PARTUUID POOL_GUID PLAN_JSON | --resume-restore PARTUUID"
        );
        let id = args[1]
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid PARTUUID"))?;
        let swap = zbm_core::resume::linux::Swap::open(id, recording)?;
        if recording {
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
