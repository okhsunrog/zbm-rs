//! Loader-owned dm-crypt lifecycle. Target initramfs mappings are verified by
//! Device directly; this ledger only authorizes mappings created by the loader.
use super::device::Device;
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};
use zfskit::runner::{Cmd, CommandRunner};

const LEDGER: &str = "/run/zbm-rs/resume-mapping.json";
fn error(s: impl ToString) -> io::Error {
    io::Error::other(s.to_string())
}

fn owned(device: &Device) -> io::Result<bool> {
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(LEDGER)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.mode() & 0o077 != 0
        || metadata.len() > 8192
    {
        return Err(error("Invalid resume mapping ledger"));
    }
    let mut bytes = Vec::new();
    file.take(8193).read_to_end(&mut bytes)?;
    let saved: Device = serde_json::from_slice(&bytes).map_err(error)?;
    if &saved != device {
        return Err(error("Resume mapping ledger belongs to a different device"));
    }
    Ok(true)
}

pub fn available(device: &Device) -> io::Result<bool> {
    if !matches!(device, Device::Luks2 { .. }) {
        return Ok(true);
    }
    device.container()?;
    match fs::symlink_metadata(device.path()) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
        Ok(_) => (),
    }
    if !owned(device)? {
        return Err(error("Resume mapping was opened outside this loader"));
    }
    device.open(false)?;
    Ok(true)
}

pub async fn unlock(device: &Device, runner: &dyn CommandRunner, secret: &[u8]) -> io::Result<()> {
    if available(device)? {
        return Ok(());
    }
    let backing = device.container()?;
    let Device::Luks2 { mapping, .. } = device else {
        return Err(error("Not an encrypted resume device"));
    };
    // Do not activate reencryption or integrity layouts which may need recovery
    // writes, additional targets or unsupported mapping lifecycle rules.
    let dump = runner
        .run(
            Cmd::new("cryptsetup")
                .args(["luksDump", "--dump-json-metadata"])
                .arg(&backing),
        )
        .await
        .map_err(error)?;
    if !dump.status.success() || dump.stdout.len() > 65536 {
        return Err(error("Cannot inspect LUKS2 layout"));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&dump.stdout).map_err(error)?;
    let segments = metadata["segments"]
        .as_object()
        .ok_or_else(|| error("Missing LUKS2 segments"))?;
    if segments.len() != 1
        || segments
            .values()
            .any(|s| s["type"] != "crypt" || s.get("integrity").is_some())
        || metadata["config"]["requirements"]["mandatory"]
            .as_array()
            .is_some_and(|r| !r.is_empty())
    {
        return Err(error(
            "Only a single ordinary LUKS2 crypt segment is supported for resume",
        ));
    }
    fs::create_dir_all(Path::new(LEDGER).parent().unwrap())?;
    if !owned(device)? {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(LEDGER)?;
        file.write_all(&serde_json::to_vec(device).map_err(error)?)?;
    }
    // The mapping permits the explicit, confirmed cancellation writer. Status
    // and handoff hold read-only FDs; it is never activated as swap by the loader.
    // No persistent activation flags, discard or metadata updates are requested.
    let output = runner
        .run(
            Cmd::new("cryptsetup")
                .args([
                    "open",
                    "--type",
                    "luks2",
                    "--batch-mode",
                    "--disable-external-tokens",
                    "--key-file",
                    "-",
                ])
                .arg(&backing)
                .arg(mapping)
                .stdin_secret(secret.to_vec()),
        )
        .await
        .map_err(error)?;
    if !output.status.success() {
        return Err(error(
            "LUKS2 swap could not be unlocked with the configured ZFS credential",
        ));
    }
    if !available(device)? {
        return Err(error(
            "LUKS2 activation did not create the expected mapping",
        ));
    }
    Ok(())
}

pub fn release(device: &Device) -> io::Result<()> {
    if !matches!(device, Device::Luks2 { .. }) {
        return Ok(());
    }
    if !available(device)? {
        return Err(error("Resume mapping disappeared before handoff"));
    }
    let Device::Luks2 { mapping, .. } = device else {
        unreachable!()
    };
    if !std::process::Command::new("cryptsetup")
        .args(["close", mapping])
        .output()?
        .status
        .success()
    {
        return Err(error("Cannot release the owned resume mapping"));
    }
    fs::remove_file(LEDGER)
}
