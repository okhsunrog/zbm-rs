//! ZFSBootMenu-compatible visibility policy; no imports or mounts here.
use crate::boot::{BootTarget, GenerationIssue};
use serde::{Deserialize, Serialize};
use std::io;
use zfskit::models::PropertyMap;

pub const PROPERTIES: &[&str] = &[
    "mountpoint",
    "canmount",
    "encryption",
    "encryptionroot",
    "keystatus",
    "keyformat",
    "keylocation",
    "org.zfsbootmenu:keysource",
    "org.zfsbootmenu:active",
    "org.zfsbootmenu:commandline",
    "org.zfsbootmenu:kernel",
    "org.zfsbootmenu:rootprefix",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootEnvironment {
    pub pool_guid: u64,
    pub dataset: String,
    pub is_default: bool,
    pub targets: Vec<BootTarget>,
    pub default_target: Option<usize>,
    pub default_error: Option<String>,
    pub rejected_generations: Vec<GenerationIssue>,
    pub unavailable: Option<String>,
    pub properties: BootProperties,
    pub root: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootProperties {
    pub mountpoint: String,
    pub canmount: String,
    pub encryption: String,
    pub encryptionroot: Option<String>,
    pub keystatus: String,
    pub keyformat: String,
    #[serde(default)]
    pub keylocation: Option<String>,
    #[serde(default)]
    pub keysource: Option<String>,
    pub active: String,
    // Retain these for future generic Linux support. Bootspec remains authoritative.
    pub commandline: Option<String>,
    pub kernel: Option<String>,
    pub rootprefix: Option<String>,
}

impl BootProperties {
    pub fn from_zfs(properties: &PropertyMap) -> io::Result<Self> {
        let required = |name: &str| {
            properties
                .get(name)
                .map(|p| p.value.clone())
                .ok_or_else(|| io::Error::other(format!("Missing BE property {name}")))
        };
        let optional = |name: &str| {
            properties
                .get(name)
                .map(|p| p.value.clone())
                .filter(|value| value != "-" && !value.is_empty())
        };
        Ok(Self {
            mountpoint: required("mountpoint")?,
            canmount: required("canmount")?,
            encryption: required("encryption")?,
            encryptionroot: optional("encryptionroot"),
            keystatus: required("keystatus")?,
            keyformat: required("keyformat")?,
            keylocation: optional("keylocation"),
            keysource: optional("org.zfsbootmenu:keysource"),
            active: required("org.zfsbootmenu:active")?,
            commandline: optional("org.zfsbootmenu:commandline"),
            kernel: optional("org.zfsbootmenu:kernel"),
            rootprefix: optional("org.zfsbootmenu:rootprefix"),
        })
    }

    pub fn visible(&self) -> bool {
        self.active != "off"
            && (self.mountpoint == "/" || (self.mountpoint == "legacy" && self.active == "on"))
    }

    pub fn unavailable(&self) -> Option<String> {
        if self.canmount == "off" {
            Some("canmount=off; dataset is not mountable".into())
        } else if self.locked() {
            Some(
                if self.can_unlock() {
                    "Locked; unlock to discover boot targets"
                } else {
                    "Locked; a key file or keysource is required"
                }
                .into(),
            )
        } else {
            None
        }
    }

    pub fn locked(&self) -> bool {
        self.encryption != "off" && self.keystatus != "available"
    }

    pub fn can_unlock(&self) -> bool {
        self.canmount != "off"
            && self.locked()
            && self.keyformat == "passphrase"
            && self.encryptionroot.is_some()
    }
}

/// A pool property selects a candidate; it never authorizes a boot.
pub fn preferred_environment(environments: &[BootEnvironment], pool: Option<u64>) -> Option<usize> {
    let mut defaults = environments
        .iter()
        .enumerate()
        .filter(|(_, e)| e.is_default && pool.is_none_or(|guid| guid == e.pool_guid));
    let first = defaults.next()?.0;
    defaults.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn properties(mountpoint: &str, active: &str) -> BootProperties {
        BootProperties {
            mountpoint: mountpoint.into(),
            canmount: "noauto".into(),
            encryption: "off".into(),
            encryptionroot: None,
            keystatus: "-".into(),
            keyformat: "-".into(),
            keylocation: None,
            keysource: None,
            active: active.into(),
            commandline: None,
            kernel: None,
            rootprefix: None,
        }
    }
    #[test]
    fn visibility_matches_zbm_root_and_legacy_rules() {
        assert!(properties("/", "-").visible());
        assert!(!properties("/", "off").visible());
        assert!(properties("legacy", "on").visible());
        assert!(!properties("legacy", "-").visible());
        assert!(!properties("none", "on").visible());
        assert!(!properties("/home", "on").visible());
        assert!(properties("/", "-").unavailable().is_none());
        let mut encrypted = properties("/", "on");
        encrypted.encryption = "aes-256-gcm".into();
        assert!(encrypted.unavailable().unwrap().contains("Locked"));
        encrypted.encryptionroot = Some("tank".into());
        encrypted.keyformat = "passphrase".into();
        assert!(encrypted.can_unlock());
        encrypted.keystatus = "available".into();
        assert!(encrypted.unavailable().is_none());
        assert!(!encrypted.can_unlock());
        encrypted.canmount = "off".into();
        assert!(encrypted.unavailable().unwrap().contains("canmount"));
    }

    #[test]
    fn inherited_properties_are_used_and_missing_native_properties_fail() {
        let mut map = PropertyMap::new();
        for (name, value) in [
            ("mountpoint", "legacy"),
            ("canmount", "noauto"),
            ("encryption", "off"),
            ("keystatus", "-"),
            ("keyformat", "-"),
            ("org.zfsbootmenu:active", "on"),
            ("org.zfsbootmenu:commandline", "quiet debug"),
        ] {
            map.insert(
                name.into(),
                serde_json::from_value(serde_json::json!({
                    "value": value, "source": {"type": "INHERITED", "data": "tank/ROOT"}
                }))
                .unwrap(),
            );
        }
        let policy = BootProperties::from_zfs(&map).unwrap();
        assert!(policy.visible());
        assert_eq!(policy.commandline.as_deref(), Some("quiet debug"));
        assert_eq!(policy.kernel, None);
        map.remove("encryption");
        assert!(
            BootProperties::from_zfs(&map)
                .unwrap_err()
                .to_string()
                .contains("encryption")
        );
    }

    #[test]
    fn a_default_is_required_and_multiple_pool_defaults_are_ambiguous() {
        let environment = |pool_guid, is_default| BootEnvironment {
            pool_guid,
            is_default,
            dataset: format!("pool{pool_guid}/root"),
            properties: properties("/", "on"),
            targets: vec![],
            default_target: None,
            default_error: None,
            rejected_generations: vec![],
            unavailable: None,
            root: None,
        };
        let mut environments = vec![environment(1, false), environment(2, false)];
        assert_eq!(preferred_environment(&environments, None), None);
        environments[0].is_default = true;
        assert_eq!(preferred_environment(&environments, None), Some(0));
        environments[1].is_default = true;
        assert_eq!(preferred_environment(&environments, None), None);
        assert_eq!(preferred_environment(&environments, Some(2)), Some(1));
        assert_eq!(preferred_environment(&environments, Some(3)), None);
    }
}
