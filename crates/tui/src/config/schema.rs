//! Shared schema and validator for the build host and runtime.
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub ui: Ui,
    pub manager: Manager,
    pub zfs: Zfs,
    pub nixos: Nixos,
    pub security: Security,
    /// Additional target kernel arguments, validated again by the boot backend.
    pub kernel_args: Vec<String>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    pub timeout_secs: u32,
    pub show_snapshots: bool,
    pub title: Option<String>,
}
impl Default for Ui {
    fn default() -> Self {
        Self {
            timeout_secs: 5,
            show_snapshots: true,
            title: None,
        }
    }
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Manager {
    pub restart_limit: u32,
}
impl Default for Manager {
    fn default() -> Self {
        Self { restart_limit: 2 }
    }
}
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Zfs {
    pub import_policy: ImportPolicy,
}
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportPolicy {
    #[default]
    HostId,
    ReadOnly,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Nixos {
    pub generation_limit: u32,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SecurityMode {
    #[default]
    Off,
    Enforce,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MeasurementPolicy {
    #[default]
    Off,
    Optional,
    Required,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Security {
    pub mode: SecurityMode,
    pub require_firmware_secure_boot: bool,
    /// Image-owned public X.509 certificates, never private key paths.
    pub target_authorities: Vec<String>,
    pub ima_certificate: Option<String>,
    pub tpm: Tpm,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tpm {
    pub policy: MeasurementPolicy,
}
impl Default for Nixos {
    fn default() -> Self {
        Self {
            generation_limit: 20,
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        self.security.validate()?;
        if self.ui.timeout_secs > 300 {
            return Err("ui.timeout_secs must be in 0..=300".into());
        }
        if self.manager.restart_limit > 8 {
            return Err("manager.restart_limit must be in 0..=8".into());
        }
        if !(1..=512).contains(&self.nixos.generation_limit) {
            return Err("nixos.generation_limit must be in 1..=512".into());
        }
        if self.kernel_args.len() > 64
            || self
                .kernel_args
                .iter()
                .any(|s| s.len() > 4096 || s.contains('\0'))
        {
            return Err(
                "kernel_args must have at most 64 entries, each <=4096 bytes without NUL".into(),
            );
        }
        if self
            .ui
            .title
            .as_ref()
            .is_some_and(|s| s.len() > 256 || s.chars().any(char::is_control))
        {
            return Err("ui.title must be <=256 bytes without control characters".into());
        }
        Ok(())
    }
}

impl Security {
    pub fn validate(&self) -> Result<(), String> {
        let public_path = |value: &str| {
            value.starts_with("/etc/zbm-rs/trust/")
                && !value.contains("..")
                && !value.chars().any(char::is_control)
                && value.len() <= 512
                && !value.ends_with('/')
        };
        if self.target_authorities.len() > 32
            || self.target_authorities.iter().any(|p| !public_path(p))
            || self
                .ima_certificate
                .as_ref()
                .is_some_and(|p| !public_path(p))
        {
            return Err(
                "security certificates must use image-owned /etc/zbm-rs/trust paths".into(),
            );
        }
        if self.mode == SecurityMode::Enforce
            && (self.target_authorities.is_empty() || self.ima_certificate.is_none())
        {
            return Err(
                "security.mode=enforce requires target authorities and an IMA certificate".into(),
            );
        }
        if self.require_firmware_secure_boot && self.mode != SecurityMode::Enforce {
            return Err("firmware Secure Boot requirement needs security.mode=enforce".into());
        }
        Ok(())
    }
}
