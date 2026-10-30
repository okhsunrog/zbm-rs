//! Identity of a dedicated swap device. Opening never unlocks, formats or
//! activates swap. A closed or mismatched LUKS mapping is an error, not absence
//! of a hibernation image.
use super::linux::valid_partuuid;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Device {
    Partition {
        partuuid: String,
    },
    Luks2 {
        partuuid: String,
        luks_uuid: String,
        mapping: String,
    },
}

fn error(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

impl Device {
    pub fn validate(&self) -> io::Result<()> {
        let partuuid = match self {
            Self::Partition { partuuid } | Self::Luks2 { partuuid, .. } => partuuid,
        };
        if !valid_partuuid(partuuid) {
            return Err(error("Invalid swap PARTUUID"));
        }
        if let Self::Luks2 {
            luks_uuid, mapping, ..
        } = self
        {
            if !valid_partuuid(luks_uuid) {
                return Err(error("Invalid LUKS UUID"));
            }
            if mapping.is_empty()
                || mapping.len() > 63
                || mapping == "control"
                || mapping.starts_with('-')
                || !mapping
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            {
                return Err(error("Invalid resume mapping name"));
            }
        }
        Ok(())
    }

    pub(crate) fn open(&self, write: bool) -> io::Result<File> {
        self.validate()?;
        let partuuid = match self {
            Self::Partition { partuuid } | Self::Luks2 { partuuid, .. } => partuuid,
        };
        let backing = fs::canonicalize(format!("/dev/disk/by-partuuid/{partuuid}"))?;
        let backing_metadata = fs::metadata(&backing)?;
        if !backing_metadata.file_type().is_block_device() {
            return Err(error("Resume backing device is not a block device"));
        }
        let path = match self {
            Self::Partition { .. } => backing.clone(),
            Self::Luks2 { mapping, .. } => {
                fs::canonicalize(Path::new("/dev/mapper").join(mapping))?
            }
        };
        let file = OpenOptions::new()
            .read(true)
            .write(write)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | if write { libc::O_EXCL } else { 0 })
            .open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_block_device() {
            return Err(error("Resume device is not a block device"));
        }
        if let Self::Luks2 {
            luks_uuid, mapping, ..
        } = self
        {
            verify_mapping(&backing, backing_metadata.rdev(), &file, luks_uuid, mapping)?;
        } else if metadata.rdev() != backing_metadata.rdev() {
            return Err(error("Resume partition changed during open"));
        }
        Ok(file)
    }
}

fn verify_mapping(
    backing: &Path,
    backing_dev: u64,
    file: &File,
    uuid: &str,
    mapping: &str,
) -> io::Result<()> {
    let dev = file.metadata()?.rdev();
    let sys = PathBuf::from(format!(
        "/sys/dev/block/{}:{}",
        libc::major(dev),
        libc::minor(dev)
    ));
    if fs::read_to_string(sys.join("dm/suspended"))?.trim() != "0" {
        return Err(error("Resume mapping is suspended"));
    }
    let expected = format!(
        "CRYPT-LUKS2-{}-{mapping}",
        uuid.replace('-', "").to_ascii_lowercase()
    );
    if fs::read_to_string(sys.join("dm/uuid"))?.trim() != expected {
        return Err(error("Resume mapping has a different LUKS identity"));
    }
    let slaves = fs::read_dir(sys.join("slaves"))?.collect::<io::Result<Vec<_>>>()?;
    let expected_dev = format!("{}:{}", libc::major(backing_dev), libc::minor(backing_dev));
    if slaves.len() != 1 || fs::read_to_string(slaves[0].path().join("dev"))?.trim() != expected_dev
    {
        return Err(error("Resume mapping uses a different backing partition"));
    }
    let is_luks2 = Command::new("cryptsetup")
        .args(["isLuks", "--type", "luks2"])
        .arg(backing)
        .output()?;
    if !is_luks2.status.success() {
        return Err(error("Resume partition is not a valid LUKS2 container"));
    }
    let identity = Command::new("cryptsetup")
        .arg("luksUUID")
        .arg(backing)
        .output()?;
    if !identity.status.success()
        || !String::from_utf8_lossy(&identity.stdout)
            .trim()
            .eq_ignore_ascii_case(uuid)
    {
        return Err(error("Resume container UUID does not match configuration"));
    }
    // Status exposes target types, never the encryption key (unlike dmsetup
    // table). Require one crypt segment; linear/foreign mappings are refused.
    let status = Command::new("dmsetup")
        .args(["status", "--noflush", mapping])
        .output()?;
    let text = String::from_utf8_lossy(&status.stdout);
    let lines = text.lines().collect::<Vec<_>>();
    if !status.status.success()
        || lines.len() != 1
        || lines[0].split_whitespace().nth(2) != Some("crypt")
    {
        return Err(error("Resume mapping is not a single active crypt target"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_ambiguous_or_path_like_identity_before_device_access() {
        let uuid = "12345678-1234-1234-1234-123456789abc";
        for mapping in ["", "../other", "/dev/dm-0", "control", "--help", "a b"] {
            let device = Device::Luks2 {
                partuuid: uuid.into(),
                luks_uuid: uuid.into(),
                mapping: mapping.into(),
            };
            // A leading dash is not a safe command argument either.
            assert!(device.validate().is_err(), "{mapping}");
        }
    }
}
