//! ZFSBootMenu-compatible visibility policy; no imports or mounts here.
use crate::boot::{BootTarget, GenerationIssue};
use serde::Serialize;
use std::io;
use zfskit::models::PropertyMap;

pub const PROPERTIES: &[&str] = &[
    "mountpoint",
    "canmount",
    "encryption",
    "org.zfsbootmenu:active",
    "org.zfsbootmenu:commandline",
    "org.zfsbootmenu:kernel",
    "org.zfsbootmenu:rootprefix",
];

#[derive(Debug, Clone, Serialize)]
pub struct BootEnvironment {
    pub dataset: String,
    pub is_default: bool,
    pub targets: Vec<BootTarget>,
    pub rejected_generations: Vec<GenerationIssue>,
    pub unavailable: Option<String>,
    pub properties: BootProperties,
    pub root: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BootProperties {
    pub mountpoint: String,
    pub canmount: String,
    pub encryption: String,
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
        } else if self.encryption != "off" {
            Some("Encrypted boot environments are not supported yet".into())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn properties(mountpoint: &str, active: &str) -> BootProperties {
        BootProperties {
            mountpoint: mountpoint.into(),
            canmount: "noauto".into(),
            encryption: "off".into(),
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
        assert!(encrypted.unavailable().unwrap().contains("Encrypted"));
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
}
