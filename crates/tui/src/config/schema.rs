//! Shared schema and validator for the build host and runtime.
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub ui: Ui,
    pub manager: Manager,
    pub zfs: Zfs,
    pub nixos: Nixos,
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
impl Default for Nixos {
    fn default() -> Self {
        Self {
            generation_limit: 20,
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
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
