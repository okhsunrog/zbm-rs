//! Broker-owned, demand-driven key dependencies. No secrets leave this module.
use crate::{Pool, Zfs, boot::rooted_path, boot_zfs, environment::BootProperties};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    future::Future,
    io::{self, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    pin::Pin,
};
use zeroize::Zeroizing;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Challenge {
    pub pool_guid: u64,
    pub encryption_root: String,
    pub reason: Option<String>,
}
#[derive(Serialize, Deserialize)]
pub struct Answer {
    pub challenge: Challenge,
    pub passphrase: Zeroizing<Vec<u8>>,
}
#[derive(Debug, Serialize, Deserialize)]
pub enum Status {
    Ready,
    Passphrase(Challenge),
}

fn error(message: impl std::fmt::Display) -> io::Error {
    io::Error::other(message.to_string())
}

/// A previous broker's process group has already been reaped by PID 1.
pub fn cleanup_cache() -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let directory = Path::new("/run/zbm-rs/keys");
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.is_dir() || metadata.uid() != 0 {
        return Err(error("Invalid broker key-cache ownership"));
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
        } else {
            return Err(error("Unexpected key-cache entry"));
        }
    }
    Ok(())
}

struct Context<'a> {
    zfs: &'a Zfs,
    pools: Vec<Pool>,
    answer: Option<Answer>,
    stack: Vec<(u64, String)>,
}

/// Validate the original boot selection before following any keysource metadata.
pub async fn resolve(
    zfs: &Zfs,
    pool: &Pool,
    dataset: &str,
    answer: Option<Answer>,
) -> io::Result<Status> {
    let pools = boot_zfs::managed_pools(zfs).await?;
    if !pools
        .iter()
        .any(|owned| owned.guid == pool.guid && owned.name == pool.name)
        || !dataset.starts_with(&format!("{}/", pool.name)) && dataset != pool.name
    {
        return Err(error("Unlock target is outside an owned pool"));
    }
    let properties = boot_zfs::properties(zfs, dataset).await?;
    if !properties.visible() || properties.canmount == "off" {
        return Err(error("Unlock target is not a mountable boot environment"));
    }
    Context {
        zfs,
        pools,
        answer,
        stack: vec![],
    }
    .unlock(dataset.to_owned())
    .await
}

impl Context<'_> {
    fn unlock(
        &mut self,
        dataset: String,
    ) -> Pin<Box<dyn Future<Output = io::Result<Status>> + Send + '_>> {
        Box::pin(async move {
            zfskit::names::DatasetName::parse(&dataset).map_err(error)?;
            let pool = self
                .pools
                .iter()
                .find(|pool| {
                    dataset == pool.name || dataset.starts_with(&format!("{}/", pool.name))
                })
                .cloned()
                .ok_or_else(|| error("Keysource pool was not imported by this manager"))?;
            let properties = boot_zfs::properties(self.zfs, &dataset).await?;
            if !properties.locked() {
                return Ok(Status::Ready);
            }
            let root = properties
                .encryptionroot
                .as_deref()
                .ok_or_else(|| error("Missing encryption root"))?
                .to_owned();
            let root_properties = boot_zfs::properties(self.zfs, &root).await?;
            if self.answer.as_ref().is_some_and(|answer| {
                answer.challenge.pool_guid == pool.guid && answer.challenge.encryption_root == root
            }) {
                let answer = self.answer.take().unwrap();
                if root_properties.keyformat != "passphrase"
                    || answer.passphrase.is_empty()
                    || answer.passphrase.len() > 1024
                    || answer
                        .passphrase
                        .iter()
                        .any(|byte| matches!(byte, 0 | b'\n' | b'\r'))
                {
                    return Err(error("Invalid passphrase response"));
                }
                zfskit::encryption::load_key_with_passphrase(
                    self.zfs.command_runner(),
                    &root,
                    &answer.passphrase,
                )
                .await
                .map_err(error)?;
                return Ok(Status::Ready);
            }
            let identity = (pool.guid, root.clone());
            if self.stack.contains(&identity) || self.stack.len() >= 32 {
                return challenge(
                    &pool,
                    &root,
                    &root_properties,
                    Some("Keysource dependency cycle; unlock this root directly".into()),
                );
            }
            self.stack.push(identity);
            let mut reason = None;
            if let Some(location) = root_properties
                .keylocation
                .as_deref()
                .filter(|location| *location != "prompt" && *location != "none")
            {
                if let Some(key) = location
                    .strip_prefix("file://")
                    .filter(|path| path.starts_with('/'))
                {
                    if let Ok(path) = rooted_path(Path::new("/"), Path::new(key))
                        && let Ok(secret) = read_key(&path)
                        && self.load_file(&root, secret).await.is_ok()
                    {
                        self.stack.pop();
                        return Ok(Status::Ready);
                    }
                    if let Some(source) = properties.keysource.as_ref() {
                        let source_result = async {
                            let source_props = boot_zfs::properties(self.zfs, source).await?;
                            if source_props.encryption == "off" {
                                return Err(error("Keysource must be encrypted"));
                            }
                            match self.unlock(source.clone()).await? {
                                Status::Passphrase(challenge) => return Ok(Some(challenge)),
                                Status::Ready => {}
                            }
                            // A source on the same encryption root may already have unlocked us.
                            if !boot_zfs::properties(self.zfs, &dataset).await?.locked() {
                                return Ok(None);
                            }
                            let source_pool = self
                                .pools
                                .iter()
                                .find(|p| {
                                    source == &p.name || source.starts_with(&format!("{}/", p.name))
                                })
                                .cloned()
                                .ok_or_else(|| error("Keysource pool is unavailable"))?;
                            let mounted =
                                boot_zfs::mount_keysource(self.zfs, &source_pool, source).await?;
                            let path = source_key(&mounted, &source_props.mountpoint, key)?;
                            self.load_file(&root, read_key(&path)?).await?;
                            Ok(None)
                        }
                        .await;
                        match source_result {
                            Ok(Some(challenge)) => {
                                self.stack.pop();
                                return Ok(Status::Passphrase(challenge));
                            }
                            Ok(None) => {
                                self.stack.pop();
                                return Ok(Status::Ready);
                            }
                            Err(error) => reason = Some(error.to_string()),
                        }
                    } else {
                        reason = Some(
                            "Key file is unavailable; enter the encryption-root passphrase".into(),
                        );
                    }
                } else {
                    reason = Some("Only local file:// keylocations are supported".into());
                }
            }
            self.stack.pop();
            challenge(&pool, &root, &root_properties, reason)
        })
    }

    async fn load_file(&self, root: &str, secret: Zeroizing<Vec<u8>>) -> io::Result<()> {
        let file = KeyFile::new(&secret)?;
        zfskit::encryption::load_key_with_keylocation(
            self.zfs.command_runner(),
            root,
            &format!("file://{}", file.0.display()),
        )
        .await
        .map_err(error)
    }
}

fn challenge(
    pool: &Pool,
    root: &str,
    properties: &BootProperties,
    reason: Option<String>,
) -> io::Result<Status> {
    if properties.keyformat != "passphrase" {
        return Err(error(reason.unwrap_or_else(|| {
            "No usable key file; this encryption root does not accept a passphrase".into()
        })));
    }
    Ok(Status::Passphrase(Challenge {
        pool_guid: pool.guid,
        encryption_root: root.into(),
        reason,
    }))
}

fn read_key(path: &Path) -> io::Result<Zeroizing<Vec<u8>>> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 4096 {
        return Err(error("Key must be a regular file of at most 4 KiB"));
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(4097));
    Read::by_ref(&mut file).take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(error("Key file grew beyond 4 KiB"));
    }
    Ok(bytes)
}

fn source_key(root: &Path, mountpoint: &str, key: &str) -> io::Result<PathBuf> {
    let key = Path::new(key);
    let relative = if mountpoint.starts_with('/') && mountpoint != "/" {
        key.strip_prefix(mountpoint)
            .ok()
            .map(|path| Path::new("/").join(path))
    } else {
        None
    };
    if let Some(relative) = relative
        && let Ok(path) = rooted_path(root, &relative)
    {
        return Ok(path);
    }
    rooted_path(root, key)
}

struct KeyFile(PathBuf);
impl KeyFile {
    fn new(bytes: &[u8]) -> io::Result<Self> {
        let directory = Path::new("/run/zbm-rs/keys");
        fs::create_dir_all(directory)?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        let id = fs::read_to_string("/proc/sys/kernel/random/uuid")?;
        if !id
            .trim()
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        {
            return Err(error("Invalid random key identity"));
        }
        let path = directory.join(id.trim());
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let key = Self(path);
        file.write_all(bytes)?;
        Ok(key)
    }
}
impl Drop for KeyFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_paths_stay_rooted_and_key_reads_are_bounded() {
        let root = std::env::temp_dir().join(format!("zbm-keysource-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("keys")).unwrap();
        fs::write(root.join("keys/key"), b"synthetic-key").unwrap();
        assert_eq!(
            source_key(&root, "/keystore", "/keystore/keys/key").unwrap(),
            root.join("keys/key")
        );
        assert_eq!(
            source_key(&root, "none", "/keys/key").unwrap(),
            root.join("keys/key")
        );
        assert_eq!(
            read_key(&root.join("keys/key")).unwrap().as_slice(),
            b"synthetic-key"
        );
        assert!(source_key(&root, "/", "/../outside").is_err());
        std::os::unix::fs::symlink("../../outside", root.join("keys/escape")).unwrap();
        assert!(source_key(&root, "none", "/keys/escape").is_err());
        fs::write(root.join("keys/large"), vec![0; 4097]).unwrap();
        assert!(read_key(&root.join("keys/large")).is_err());
        assert!(read_key(&root.join("keys")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
