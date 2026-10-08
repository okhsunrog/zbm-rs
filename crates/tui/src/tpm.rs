//! Optional/required TPM evidence. TPM failures never weaken target verification.
use crate::config::{MeasurementPolicy, Tpm};
use serde::Serialize;
use std::{
    fs, io,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const EXTEND: &str = "/usr/lib/systemd/systemd-pcrextend";
const SETUP: &str = "/usr/lib/systemd/systemd-tpm2-setup";

#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    pub state: &'static str,
    pub srk_ready: bool,
    pub enter_initrd_measured: bool,
    pub initialized_nvpcrs: Vec<String>,
    pub errors: Vec<String>,
}

fn helper(program: &str, args: &[&str], name: &str) -> io::Result<bool> {
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("/run/zbm-rs/tpm-{name}.log"))?;
    let mut child = Command::new(program)
        .args(args)
        .env_clear()
        .env("LD_LIBRARY_PATH", "/usr/lib")
        .env("SYSTEMD_LOG_TARGET", "console")
        // Explicit image policy requests measurement even when firmware cannot
        // expose the stub variable. This does not assert that the UKI was measured;
        // the signed PCR policy must independently authorize the actual PCR state.
        .env("SYSTEMD_FORCE_MEASURE", "1")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status.success()),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                break Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "TPM helper deadline expired",
                ));
            }
            Err(error) => break Err(error),
        }
    };
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

pub fn prepare(config: &Tpm) -> io::Result<Evidence> {
    let mut evidence = Evidence {
        state: "disabled",
        srk_ready: false,
        enter_initrd_measured: false,
        initialized_nvpcrs: Vec::new(),
        errors: Vec::new(),
    };
    if config.policy == MeasurementPolicy::Off {
        return Ok(evidence);
    }
    evidence.state = "unavailable";
    let available = Path::new("/dev/tpmrm0").exists()
        && fs::read_to_string("/sys/class/tpm/tpm0/tpm_version_major")
            .is_ok_and(|value| value.trim() == "2");
    if !available {
        evidence
            .errors
            .push("TPM2 resource manager device is unavailable".into());
    } else {
        let phase = helper(
            EXTEND,
            &[
                "--bank=sha256",
                "--pcr=11",
                "--tpm2-device=/dev/tpmrm0",
                "enter-initrd",
            ],
            "enter-initrd",
        );
        evidence.enter_initrd_measured = matches!(phase, Ok(true));
        if !matches!(phase, Ok(true)) {
            evidence
                .errors
                .push(format!("Cannot measure enter-initrd: {phase:?}"));
        }
        let setup = helper(
            SETUP,
            &["--early=yes", "--tpm2-device=/dev/tpmrm0"],
            "setup",
        );
        evidence.srk_ready = Path::new("/run/systemd/tpm2-srk-public-key.pem").is_file();
        if !matches!(setup, Ok(true)) || !evidence.srk_ready {
            evidence.errors.push(format!(
                "TPM setup reported failure: {setup:?}; SRK ready: {}",
                evidence.srk_ready
            ));
        }
        for name in &config.nvpcrs {
            let auth = fs::read_to_string(format!("/run/systemd/nvpcr/{name}.auth"));
            if auth.is_ok_and(|value| {
                value.trim().len() == 64
                    && value.trim().bytes().all(|byte| byte.is_ascii_hexdigit())
            }) {
                evidence.initialized_nvpcrs.push(name.clone());
            } else {
                evidence
                    .errors
                    .push(format!("Selected NvPCR '{name}' could not be initialized"));
            }
        }
        if evidence
            .initialized_nvpcrs
            .iter()
            .any(|name| name == "hardware")
            && !matches!(
                helper(
                    EXTEND,
                    &["--tpm2-device=/dev/tpmrm0", "--product-id"],
                    "hardware"
                ),
                Ok(true)
            )
        {
            evidence
                .errors
                .push("Hardware identity measurement failed".into());
            evidence
                .initialized_nvpcrs
                .retain(|name| name != "hardware");
        }
        evidence.state = if evidence.errors.is_empty() {
            "ready"
        } else {
            "degraded"
        };
    }
    let required_nv_missing = config
        .required_nvpcrs
        .iter()
        .any(|name| !evidence.initialized_nvpcrs.contains(name));
    fs::write("/run/zbm-rs/tpm.json", serde_json::to_vec(&evidence)?)?;
    if required_nv_missing
        || (config.policy == MeasurementPolicy::Required
            && (!evidence.srk_ready || !evidence.enter_initrd_measured))
    {
        return Err(io::Error::other(format!(
            "Required TPM capability failed: {}",
            evidence.errors.join("; ")
        )));
    }
    Ok(evidence)
}

pub fn prepared_target(verified: &zbm_core::security::VerifiedBootPlan) -> io::Result<()> {
    use zbm_core::security::Artifact;
    let config = crate::config::load(true).map_err(|e| io::Error::other(e.to_string()))?;
    if config.security.tpm.policy == MeasurementPolicy::Off {
        return Ok(());
    }
    // This records a verified prepared attempt, not proof of successful execution.
    let record = serde_json::to_vec(&serde_json::json!({ "version": 1,
        "inputs": verified.evidence(), "arguments": verified.arguments() }))?;
    let path = "/run/zbm-rs/tpm-target.json";
    fs::write(path, record)?;
    let digest = Artifact::read(Path::new(path))?.sha256;
    let word = format!("zbm-rs:target-prepared:v1:{digest}");
    let measured = helper(
        EXTEND,
        &[
            "--bank=sha256",
            "--pcr=15",
            "--tpm2-device=/dev/tpmrm0",
            &word,
        ],
        "target",
    );
    if !matches!(measured, Ok(true)) {
        fs::write("/run/zbm-rs/tpm-target-error", format!("{measured:?}"))?;
        if config.security.tpm.policy == MeasurementPolicy::Required {
            return Err(io::Error::other("Required target TPM measurement failed"));
        }
    }
    Ok(())
}
