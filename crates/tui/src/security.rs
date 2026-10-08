//! Trusted early setup. Mandatory protection is independent of TPM availability.
use crate::config::{Config, SecurityMode};
use serde::Serialize;
use std::{ffi::CString, fs, io, os::fd::AsRawFd, sync::OnceLock};

static EVIDENCE: OnceLock<Evidence> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FirmwareState {
    Enabled,
    Disabled,
    Unknown,
}

pub fn firmware_from_bytes(bytes: &[u8]) -> FirmwareState {
    match bytes {
        [_, _, _, _, 1] => FirmwareState::Enabled,
        [_, _, _, _, 0] => FirmwareState::Disabled,
        _ => FirmwareState::Unknown,
    }
}

pub fn firmware_state() -> FirmwareState {
    fs::read("/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c")
        .map_or(FirmwareState::Unknown, |data| firmware_from_bytes(&data))
}

pub fn ready() -> bool {
    EVIDENCE.get().is_some()
}

#[derive(Clone, Serialize)]
pub struct Evidence {
    pub firmware: FirmwareState,
    pub mode: SecurityMode,
    pub lockdown: Option<String>,
    pub ima_policy_loaded: bool,
    pub tpm: crate::tpm::Evidence,
}

fn error(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

fn ima_keyring(keys: &str) -> io::Result<libc::c_long> {
    let mut ids = keys.lines().filter_map(|line| {
        let fields: Vec<_> = line.split_whitespace().collect();
        (fields.get(7) == Some(&"keyring")
            && fields.get(8) == Some(&".ima:")
            && fields.get(5) == Some(&"0"))
        .then(|| i32::from_str_radix(fields[0], 16).ok())
        .flatten()
    });
    match (ids.next(), ids.next()) {
        (Some(id), None) if id > 0 => Ok(id.into()),
        _ => Err(error(
            "Existing kernel .ima keyring is missing or ambiguous",
        )),
    }
}

pub fn prepare(config: &Config) -> io::Result<Evidence> {
    if let Some(evidence) = EVIDENCE.get() {
        return Ok(evidence.clone());
    }
    let firmware = firmware_state();
    let mut evidence = Evidence {
        firmware,
        mode: config.security.mode,
        lockdown: None,
        ima_policy_loaded: false,
        tpm: crate::tpm::Evidence {
            state: "disabled",
            srk_ready: false,
            enter_initrd_measured: false,
            initialized_nvpcrs: Vec::new(),
            errors: Vec::new(),
        },
    };
    if config.security.mode == SecurityMode::Enforce {
        if config.security.require_firmware_secure_boot && firmware != FirmwareState::Enabled {
            return Err(error("Firmware Secure Boot is disabled or unknown"));
        }
        let lockdown = fs::read_to_string("/sys/kernel/security/lockdown")?;
        if !lockdown.contains("[integrity]") && !lockdown.contains("[confidentiality]") {
            return Err(error("Required kernel lockdown is inactive"));
        }
        evidence.lockdown = Some(lockdown.trim().into());
        let module_enforcement = fs::read_to_string("/sys/module/module/parameters/sig_enforce")?;
        if module_enforcement.trim() != "Y" && module_enforcement.trim() != "1" {
            return Err(error("Required module signature enforcement is inactive"));
        }
        load_ima_policy(config)?;
        evidence.ima_policy_loaded = true;
    }
    evidence.tpm = crate::tpm::prepare(&config.security.tpm)?;
    fs::write(
        "/run/zbm-rs/security.json",
        serde_json::to_vec(&evidence).map_err(|e| error(e.to_string()))?,
    )?;
    let _ = EVIDENCE.set(evidence.clone());
    Ok(evidence)
}

fn load_ima_policy(config: &Config) -> io::Result<()> {
    let certificate = config
        .security
        .ima_certificate
        .as_ref()
        .ok_or_else(|| error("Missing IMA certificate"))?;
    let certificate = fs::read(certificate)?;
    if certificate.len() > 64 * 1024 {
        return Err(error("Oversized IMA certificate"));
    }
    // Kernel-owned .ima is not necessarily linked to our process keyrings, so
    // request_key() cannot reliably find it. Resolve its public serial from
    // procfs metadata, as keyutils does; never create a replacement ring.
    let ring = ima_keyring(&fs::read_to_string("/proc/keys")?)?;
    let asymmetric = CString::new("asymmetric").unwrap();
    let description = CString::new("").unwrap();
    let key = unsafe {
        libc::syscall(
            libc::SYS_add_key,
            asymmetric.as_ptr(),
            description.as_ptr(),
            certificate.as_ptr(),
            certificate.len(),
            ring,
        )
    };
    if key < 0 {
        return Err(error(format!(
            "Cannot add IMA certificate to restricted .ima keyring: {}",
            io::Error::last_os_error()
        )));
    }
    let policy_path = "/etc/zbm-rs/security/ima-policy";
    let signature = fs::read(format!("{policy_path}.sig"))?;
    if !(9..=8192).contains(&signature.len()) {
        return Err(error("Missing or oversized IMA policy signature"));
    }
    let file = fs::File::open(policy_path)?;
    let attribute = CString::new("security.ima").unwrap();
    if unsafe {
        libc::fsetxattr(
            file.as_raw_fd(),
            attribute.as_ptr(),
            signature.as_ptr().cast(),
            signature.len(),
            0,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let securityfs = [
        "/sys/kernel/security/integrity/ima/policy",
        "/sys/kernel/security/ima/policy",
    ]
    .into_iter()
    .find(|path| std::path::Path::new(path).exists())
    .ok_or_else(|| error("IMA policy interface unavailable"))?;
    fs::write(securityfs, format!("{policy_path}\n"))?;
    let loaded = fs::read_to_string(securityfs)?;
    if !loaded.lines().any(|line| {
        line.split_whitespace().collect::<Vec<_>>()
            == [
                "appraise",
                "func=KEXEC_INITRAMFS_CHECK",
                "appraise_type=imasig",
            ]
    }) {
        return Err(error("Mandatory initramfs appraisal rule missing"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ima_lookup_uses_existing_kernel_ring_and_rejects_ambiguity() {
        let line = "3ef23263 I------ 1 perm 1f0f0000 0 0 keyring .ima: 1\n";
        assert_eq!(ima_keyring(line).unwrap(), 0x3ef23263);
        assert!(ima_keyring("").is_err());
        assert!(ima_keyring(&format!("{line}{line}")).is_err());
        assert!(ima_keyring(&line.replace("keyring .ima:", "user .ima:")).is_err());
        assert!(ima_keyring(&line.replace(" 0 0 keyring", " 1000 0 keyring")).is_err());
    }
    #[test]
    fn firmware_attributes_are_not_the_secure_boot_value() {
        assert_eq!(
            firmware_from_bytes(&[6, 0, 0, 0, 1]),
            FirmwareState::Enabled
        );
        assert_eq!(
            firmware_from_bytes(&[6, 0, 0, 0, 0]),
            FirmwareState::Disabled
        );
        for bytes in [
            &[][..],
            &[6, 0, 0, 0][..],
            &[6, 0, 0, 0, 2][..],
            &[6, 0, 0, 0, 1, 1][..],
        ] {
            assert_eq!(firmware_from_bytes(bytes), FirmwareState::Unknown);
        }
    }
}
