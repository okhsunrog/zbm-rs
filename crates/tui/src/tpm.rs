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

#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    pub state: &'static str,
    /// Read-only readiness probe; PCR11 phases, SRK and NvPCR belong to the OS.
    pub pcr15_initial: Option<String>,
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
        // expose the stub variable. This override does not prove UKI measurement
        // or attestation; it only permits the explicit prepared-target extend.
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
        pcr15_initial: None,
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
        // Reading this sysfs attribute performs TPM PCR_Read. Do not create a
        // persistent SRK, allocate NV indices or advance the OS's PCR11 phases.
        match fs::read_to_string("/sys/class/tpm/tpm0/pcr-sha256/15") {
            Ok(value)
                if value.trim().len() == 64
                    && value.trim().bytes().all(|byte| byte.is_ascii_hexdigit()) =>
            {
                evidence.pcr15_initial = Some(value.trim().to_ascii_lowercase());
                evidence.state = "ready";
            }
            result => {
                evidence.state = "degraded";
                evidence
                    .errors
                    .push(format!("Cannot read SHA-256 PCR15: {result:?}"));
            }
        }
    }
    fs::write("/run/zbm-rs/tpm.json", serde_json::to_vec(&evidence)?)?;
    if config.policy == MeasurementPolicy::Required && evidence.pcr15_initial.is_none() {
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
