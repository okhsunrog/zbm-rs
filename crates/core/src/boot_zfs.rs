//! Explicit user-selected imports and read-only mounts, with restart reconciliation.
use crate::{
    Pool, Zfs,
    boot::{self, BootTarget},
};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use zfskit::{
    Cmd,
    dataset::ListOptions,
    models::DatasetType,
    pool::{DiscoveredPool, ImportOptions},
};

#[derive(Serialize, Deserialize)]
struct Lease {
    name: String,
    guid: u64,
    mounts: Vec<(String, PathBuf)>,
}
fn failure(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}
async fn command(zfs: &Zfs, cmd: Cmd) -> io::Result<()> {
    let output = zfs.command_runner().run(cmd).await.map_err(failure)?;
    if !output.status.success() {
        return Err(failure(String::from_utf8_lossy(&output.stderr)));
    }
    Ok(())
}
fn ledger(pool: &Pool) -> PathBuf {
    PathBuf::from(format!("/run/zbm-rs/managed/{}.json", pool.guid))
}
fn save(pool: &Pool, lease: &Lease) -> io::Result<()> {
    let path = ledger(pool);
    fs::create_dir_all(path.parent().unwrap())?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, serde_json::to_vec(lease).map_err(failure)?)?;
    fs::rename(temporary, path)
}
fn load(pool: &Pool) -> io::Result<Lease> {
    let lease: Lease = serde_json::from_slice(&fs::read(ledger(pool))?).map_err(failure)?;
    if lease.name != pool.name || lease.guid != pool.guid {
        return Err(failure("Managed pool identity mismatch"));
    }
    Ok(lease)
}
pub async fn managed_pools(zfs: &Zfs) -> io::Result<Vec<Pool>> {
    let directory = match fs::read_dir("/run/zbm-rs/managed") {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error),
    };
    let imported = zfs.list_pools(&Default::default()).await.map_err(failure)?;
    let mut pools = vec![];
    for entry in directory {
        let path = entry?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let lease: Lease = serde_json::from_slice(&fs::read(path)?).map_err(failure)?;
        if let Some(pool) = imported
            .iter()
            .find(|p| p.name == lease.name && p.pool_guid.parse::<u64>().ok() == Some(lease.guid))
        {
            pools.push(Pool {
                name: pool.name.clone(),
                guid: lease.guid,
                health: pool.state.clone(),
            });
        }
    }
    Ok(pools)
}
fn mount_source(path: &Path) -> io::Result<Option<String>> {
    for line in fs::read_to_string("/proc/self/mountinfo")?.lines() {
        if let Some((before, after)) = line.split_once(" - ")
            && before.split_whitespace().nth(4) == path.to_str()
        {
            let mut fields = after.split_whitespace();
            if fields.next() != Some("zfs") {
                return Err(failure("Boot mount is occupied by another filesystem"));
            }
            if !before
                .split_whitespace()
                .nth(5)
                .is_some_and(|flags| flags.split(',').any(|flag| flag == "ro"))
            {
                return Err(failure("Existing boot mount is not read-only"));
            }
            return Ok(fields.next().map(str::to_owned));
        }
    }
    Ok(None)
}
pub async fn targets(
    zfs: &Zfs,
    pool: &Pool,
    readonly: bool,
    limit: u32,
) -> io::Result<Vec<BootTarget>> {
    let imported = zfs.list_pools(&Default::default()).await.map_err(failure)?;
    let existing = imported
        .iter()
        .find(|p| p.pool_guid.parse::<u64>().ok() == Some(pool.guid));
    let mut lease = if existing.is_some() {
        // Never claim or later export a pool imported outside this manager.
        load(pool)
            .map_err(|_| failure("Pool is already imported outside zbm-rs; refusing ownership"))?
    } else {
        let lease = Lease {
            name: pool.name.clone(),
            guid: pool.guid,
            mounts: vec![],
        };
        // Record intent before the effect; the next generation checks actual state.
        save(pool, &lease)?;
        let mut options = ImportOptions::new()
            .no_mount()
            .property("cachefile", "none");
        if readonly {
            options = options.readonly();
        }
        if let Err(error) = zfs
            .import_pool(
                &DiscoveredPool {
                    name: pool.name.clone(),
                    id: pool.guid.to_string(),
                    state: pool.health.clone(),
                    status: None,
                },
                &options,
            )
            .await
        {
            // A returned failure must not leave ownership of a later external import.
            let _ = fs::remove_file(ledger(pool));
            return Err(failure(error));
        }
        lease
    };
    let datasets = zfs
        .list_datasets(&ListOptions {
            recursive: true,
            roots: vec![pool.name.clone()],
            types: vec![DatasetType::Filesystem],
            ..Default::default()
        })
        .await
        .map_err(failure)?;
    let mut targets = vec![];
    for dataset in datasets {
        // First backend supports unencrypted filesystems and legacy mountpoints.
        if !dataset
            .name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_-.:".contains(&b))
        {
            return Err(failure("Unsupported dataset name in boot backend"));
        }
        let component: String = dataset.name.bytes().map(|b| format!("{b:02x}")).collect();
        let root = PathBuf::from(format!("/run/zbm-rs/roots/{}/{component}", pool.guid));
        fs::create_dir_all(&root)?;
        if let Some(source) = mount_source(&root)? {
            if source != dataset.name {
                return Err(failure("Boot mount dataset mismatch"));
            }
        } else {
            if !lease
                .mounts
                .iter()
                .any(|(name, path)| name == &dataset.name && path == &root)
            {
                lease.mounts.push((dataset.name.clone(), root.clone()));
                save(pool, &lease)?;
            }
            command(
                zfs,
                Cmd::new("mount")
                    .args(["-t", "zfs", "-o", "ro,zfsutil"])
                    .arg(&dataset.name)
                    .arg(root.as_os_str()),
            )
            .await?;
        }
        targets.extend(boot::generations(&root, &dataset.name, limit)?);
    }
    Ok(targets)
}

pub async fn release(zfs: &Zfs, pool: &Pool) -> io::Result<()> {
    let lease = load(pool)?;
    // Validate every recorded resource before performing any unmount.
    for (dataset, path) in &lease.mounts {
        if mount_source(path)?.is_some_and(|source| source != *dataset) {
            return Err(failure(
                "Owned mount was replaced by another dataset; refusing cleanup",
            ));
        }
    }
    // Export must not implicitly unmount someone else's filesystems.
    for line in fs::read_to_string("/proc/self/mountinfo")?.lines() {
        if let Some((before, after)) = line.split_once(" - ") {
            let fields: Vec<_> = after.split_whitespace().collect();
            if fields.first() == Some(&"zfs")
                && fields.get(1).is_some_and(|name| {
                    **name == pool.name || name.starts_with(&format!("{}/", pool.name))
                })
            {
                let path = before.split_whitespace().nth(4).unwrap_or("");
                if !lease
                    .mounts
                    .iter()
                    .any(|(_, owned)| owned.to_str() == Some(path))
                {
                    return Err(failure("Pool has a foreign mount; refusing export"));
                }
            }
        }
    }
    for (dataset, path) in lease.mounts.iter().rev() {
        if let Some(source) = mount_source(path)? {
            if source != *dataset {
                return Err(failure("Owned mount changed during cleanup"));
            }
            command(zfs, Cmd::new("umount").arg(path.as_os_str())).await?;
        }
    }
    // Recheck identity immediately before exporting by name.
    let imported = zfs.list_pools(&Default::default()).await.map_err(failure)?;
    if !imported
        .iter()
        .any(|p| p.name == pool.name && p.pool_guid.parse::<u64>().ok() == Some(pool.guid))
    {
        return Err(failure("Pool identity changed before export"));
    }
    zfs.pool(&pool.name)
        .map_err(failure)?
        .export(&Default::default())
        .await
        .map_err(failure)?;
    fs::remove_file(ledger(pool))
}

pub async fn release_owned(zfs: &Zfs, selected: &Pool) -> io::Result<()> {
    let pools = managed_pools(zfs).await?;
    if !pools
        .iter()
        .any(|pool| pool.guid == selected.guid && pool.name == selected.name)
    {
        return Err(failure("Selected pool is no longer owned by this manager"));
    }
    for pool in pools {
        release(zfs, &pool).await?;
    }
    Ok(())
}
