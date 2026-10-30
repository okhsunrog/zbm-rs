//! Non-secret, image-owned resume policy shared with the target initramfs.
use super::device::Device;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Rescue inspects/cancels an image but may never restore or create one.
    pub role: Role,
    /// Compatibility with existing plain-partition images.
    pub swap_partuuid: Option<String>,
    pub device: Option<Device>,
    pub credential: Option<CredentialSource>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    #[default]
    Resume,
    CancelOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CredentialSource {
    ZfsUnlock {
        // GUIDs may exceed Nix's signed integer range.
        pool_guid: String,
        encryption_root: String,
    },
}

impl CredentialSource {
    pub fn target(&self) -> (u64, &str) {
        match self {
            Self::ZfsUnlock {
                pool_guid,
                encryption_root,
            } => (pool_guid.parse().unwrap_or(0), encryption_root),
        }
    }
    pub fn matches(&self, guid: u64, root: &str) -> bool {
        self.target() == (guid, root)
    }
}

impl Config {
    pub fn apply_role(&self, status: super::Status) -> super::Status {
        match status {
            super::Status::Pending { session } if self.role == Role::CancelOnly => {
                super::Status::Unavailable {
                    session_id: session.id,
                    reason: "Rescue cannot resume; discard the saved session before booting or modifying pools".into(),
                }
            }
            status => status,
        }
    }
    pub fn enabled(&self) -> bool {
        self.device.is_some() || self.swap_partuuid.is_some()
    }
    pub fn device(&self) -> Option<Device> {
        self.device.clone().or_else(|| {
            self.swap_partuuid.as_ref().map(|id| Device::Partition {
                partuuid: id.clone(),
            })
        })
    }
    pub fn validate(&self) -> io::Result<()> {
        let invalid = |s| io::Error::other(s);
        if self.device.is_some() && self.swap_partuuid.is_some() {
            return Err(invalid(
                "Set resume.device or legacy resume.swap_partuuid, not both",
            ));
        }
        if let Some(device) = self.device() {
            device.validate()?;
            match (&device, &self.credential) {
                (Device::Luks2 { .. }, Some(source)) => {
                    let (guid, root) = source.target();
                    if guid == 0 || zfskit::names::DatasetName::parse(root).is_err() {
                        return Err(invalid(
                            "Invalid resume credential pool GUID or encryption root",
                        ));
                    }
                }
                (Device::Luks2 { .. }, None) => {
                    return Err(invalid(
                        "LUKS2 resume requires an explicit credential source",
                    ));
                }
                (Device::Partition { .. }, Some(_)) => {
                    return Err(invalid("Plain swap cannot have a credential source"));
                }
                _ => (),
            }
        } else if self.credential.is_some() {
            return Err(invalid("Resume credential requires a device"));
        }
        Ok(())
    }
}
