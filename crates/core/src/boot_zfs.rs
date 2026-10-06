//! Explicit user-selected imports and read-only mounts, with restart reconciliation.
use crate::{
    Pool, Zfs, boot,
    environment::{BootEnvironment, BootProperties, PROPERTIES},
};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use zfskit::{
    Cmd,
    dataset::GetOptions,
    models::DatasetType,
    pool::{DiscoveredPool, ImportOptions},
};

#[derive(Serialize, Deserialize)]
struct Lease {
    name: String,
    guid: u64,
    mounts: Vec<(String, PathBuf)>,
    #[serde(default)]
    clones: Vec<CloneLease>,
}
#[derive(Serialize, Deserialize, Clone)]
struct CloneLease {
    dataset: String,
    source: boot::SnapshotSource,
    token: String,
    handed_off: bool,
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

async fn mount_environment(
    zfs: &Zfs,
    pool: &Pool,
    lease: &mut Lease,
    environment: &BootEnvironment,
) -> io::Result<PathBuf> {
    let dataset = &environment.dataset;
    if !dataset
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/_-.:".contains(&b))
    {
        return Err(failure("Unsupported dataset name in boot backend"));
    }
    let component: String = dataset.bytes().map(|b| format!("{b:02x}")).collect();
    let root = PathBuf::from(format!("/run/zbm-rs/roots/{}/{component}", pool.guid));
    fs::create_dir_all(&root)?;
    if let Some(source) = mount_source(&root)? {
        if source != *dataset {
            return Err(failure("Boot mount dataset mismatch"));
        }
    } else {
        if !lease
            .mounts
            .iter()
            .any(|(name, path)| name == dataset && path == &root)
        {
            lease.mounts.push((dataset.clone(), root.clone()));
            save(pool, lease)?;
        }
        command(
            zfs,
            Cmd::new("mount")
                .args([
                    "-t",
                    "zfs",
                    "-o",
                    if environment.properties.mountpoint == "legacy" {
                        "ro"
                    } else {
                        "ro,zfsutil"
                    },
                ])
                .arg(dataset)
                .arg(root.as_os_str()),
        )
        .await?;
    }
    Ok(root)
}

async fn read_targets(
    root: &Path,
    dataset: &str,
    limit: u32,
    properties: &BootProperties,
) -> io::Result<boot::TargetDiscovery> {
    let root = root.to_owned();
    let dataset = dataset.to_owned();
    let properties = properties.clone();
    tokio::task::spawn_blocking(move || {
        let nixos = boot::discover_generations(&root, &dataset, limit)?;
        if !nixos.targets.is_empty() || !nixos.rejected.is_empty() {
            Ok(nixos)
        } else {
            crate::linux::discover(&root, &dataset, &properties)
        }
    })
    .await
    .map_err(failure)?
}

async fn properties(zfs: &Zfs, dataset: &str) -> io::Result<BootProperties> {
    let entries = zfskit::dataset::get(
        zfs.command_runner(),
        &GetOptions {
            datasets: vec![dataset.into()],
            properties: PROPERTIES.iter().map(|p| (*p).into()).collect(),
            ..Default::default()
        },
    )
    .await
    .map_err(failure)?;
    let mut properties = BootProperties::from_zfs(
        &entries
            .first()
            .ok_or_else(|| failure("Boot dataset disappeared"))?
            .properties,
    )?;
    if properties
        .commandline
        .as_ref()
        .is_some_and(|value| value.contains("%{parent}"))
    {
        properties.commandline = Some(expanded_commandline(zfs, dataset).await?);
    }
    Ok(properties)
}

async fn expanded_commandline(zfs: &Zfs, dataset: &str) -> io::Result<String> {
    let mut current = dataset.to_owned();
    let mut stack = vec![];
    let mut expanded = String::new();
    for _ in 0..128 {
        let property = zfskit::dataset::get_property(
            zfs.command_runner(),
            &current,
            "org.zfsbootmenu:commandline",
        )
        .await
        .map_err(failure)?;
        if property.value.len() > 65536 {
            return Err(failure("Kernel commandline property is too large"));
        }
        if !property.value.contains("%{parent}") {
            expanded = if property.value == "-" {
                String::new()
            } else {
                property.value
            };
            break;
        }
        stack.push((
            property.value,
            property.source.kind == zfskit::models::PropertySourceKind::Inherited,
        ));
        if let Some((parent, _)) = current.rsplit_once('/') {
            current = parent.into();
        } else {
            break;
        }
    }
    for (value, inherited) in stack.into_iter().rev() {
        if !inherited {
            expanded = value.replace("%{parent}", &expanded);
        }
        if expanded.len() > 65536 {
            return Err(failure("Expanded kernel commandline is too large"));
        }
    }
    Ok(expanded)
}

pub async fn refresh_target(zfs: &Zfs, target: &boot::BootTarget) -> io::Result<boot::BootTarget> {
    let properties = properties(zfs, &target.dataset).await?;
    if matches!(target.backend, boot::Backend::Nixos) {
        let mut refreshed = target.clone();
        refreshed.mountpoint = Some(properties.mountpoint);
        return Ok(refreshed);
    }
    let mut refreshed = read_targets(&target.root, &target.dataset, u32::MAX, &properties)
        .await?
        .targets
        .into_iter()
        .find(|t| {
            t.inputs.kernel == target.inputs.kernel && t.inputs.initrd == target.inputs.initrd
        })
        .ok_or_else(|| failure("Selected Linux kernel/initramfs pair changed"))?;
    refreshed.snapshot = target.snapshot.clone();
    Ok(refreshed)
}
pub async fn environments(
    zfs: &Zfs,
    pool: &Pool,
    readonly: bool,
    limit: u32,
) -> io::Result<Vec<BootEnvironment>> {
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
            clones: vec![],
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
    let bootfs = zfs
        .pool(&pool.name)
        .map_err(failure)?
        .get_property("bootfs")
        .await
        .map_err(failure)?
        .value;
    let datasets = zfskit::dataset::get(
        zfs.command_runner(),
        &GetOptions {
            recursive: true,
            datasets: vec![pool.name.clone()],
            properties: PROPERTIES.iter().map(|p| (*p).into()).collect(),
            types: vec![DatasetType::Filesystem],
            ..Default::default()
        },
    )
    .await
    .map_err(failure)?;
    let mut environments = vec![];
    for dataset in datasets {
        let mut properties = BootProperties::from_zfs(&dataset.properties)?;
        if properties
            .commandline
            .as_ref()
            .is_some_and(|value| value.contains("%{parent}"))
        {
            properties.commandline = Some(expanded_commandline(zfs, &dataset.name).await?);
        }
        if !properties.visible() {
            continue;
        }
        let mut environment = BootEnvironment {
            is_default: dataset.name == bootfs,
            dataset: dataset.name.clone(),
            unavailable: properties.unavailable(),
            targets: vec![],
            rejected_generations: vec![],
            properties,
            root: None,
        };
        if environment.unavailable.is_some() {
            environments.push(environment);
            continue;
        }
        let root = match mount_environment(zfs, pool, &mut lease, &environment).await {
            Ok(root) => root,
            Err(error) => {
                environment.unavailable = Some(format!("Boot environment mount failed: {error}"));
                environments.push(environment);
                continue;
            }
        };
        match read_targets(&root, &dataset.name, limit, &environment.properties).await {
            Ok(discovery) => {
                environment.unavailable = discovery
                    .targets
                    .is_empty()
                    .then(|| "No usable Linux kernel/initramfs pair or NixOS generation".into());
                environment.targets = discovery.targets;
                environment.rejected_generations = discovery.rejected;
            }
            Err(error) => {
                environment.unavailable = Some(format!("Generation discovery failed: {error}"))
            }
        }
        environment.root = Some(root);
        environments.push(environment);
    }
    environments.sort_by(|a, b| a.dataset.cmp(&b.dataset));
    Ok(environments)
}

pub async fn release(zfs: &Zfs, pool: &Pool) -> io::Result<()> {
    unmount_owned(zfs, pool).await?;
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

async fn unmount_owned(zfs: &Zfs, pool: &Pool) -> io::Result<()> {
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
                    **name == pool.name
                        || name.starts_with(&format!("{}/", pool.name))
                        || name.starts_with(&format!("{}@", pool.name))
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
    Ok(())
}

async fn writable(zfs: &Zfs, pool: &Pool) -> io::Result<()> {
    if !managed_pools(zfs)
        .await?
        .iter()
        .any(|p| p.guid == pool.guid && p.name == pool.name)
    {
        return Err(failure("Pool is not owned by this manager"));
    }
    let handle = zfs.pool(&pool.name).map_err(failure)?;
    if handle
        .get_property("readonly")
        .await
        .map_err(failure)?
        .value
        != "off"
    {
        return Err(failure(
            "Operation requires a writable import; read-only policy is preserved",
        ));
    }
    let policy = handle
        .get_property("org.zfsbootmenu:readonly")
        .await
        .map_err(failure)?
        .value;
    if policy != "-" && policy != "off" {
        return Err(failure("org.zfsbootmenu:readonly forbids mutation"));
    }
    Ok(())
}

async fn snapshot_identity(
    zfs: &Zfs,
    pool: &Pool,
    source: &boot::SnapshotSource,
) -> io::Result<String> {
    let name = zfskit::names::SnapshotName::parse(&source.name).map_err(failure)?;
    let dataset = name.dataset().as_str().to_owned();
    if dataset != pool.name && !dataset.starts_with(&format!("{}/", pool.name)) {
        return Err(failure("Snapshot is outside selected pool"));
    }
    let guid = zfskit::dataset::get_property(zfs.command_runner(), &source.name, "guid")
        .await
        .map_err(failure)?;
    if guid.value.parse::<u64>().map_err(failure)? != source.guid {
        return Err(failure("Snapshot identity changed"));
    }
    Ok(dataset)
}

/// Explicit confirmed rollback of exactly one dataset. No Bootspec requirement,
/// recursive dataset rollback, forced unmount or dependent-clone destruction.
pub async fn rollback(zfs: &Zfs, pool: &Pool, source: &boot::SnapshotSource) -> io::Result<()> {
    writable(zfs, pool).await?;
    let dataset = snapshot_identity(zfs, pool, source).await?;
    let txg = zfskit::dataset::get_property(zfs.command_runner(), &source.name, "createtxg")
        .await
        .map_err(failure)?
        .value
        .parse::<u64>()
        .map_err(failure)?;
    let entries = zfs
        .list_datasets(&zfskit::dataset::ListOptions {
            recursive: true,
            depth: Some(1),
            roots: vec![dataset.clone()],
            types: vec![DatasetType::Snapshot],
            properties: vec!["name".into(), "createtxg".into(), "clones".into()],
        })
        .await
        .map_err(failure)?;
    for entry in entries {
        if entry.name.split('@').next() == Some(dataset.as_str())
            && entry
                .properties
                .get("createtxg")
                .ok_or_else(|| failure("Missing snapshot transaction"))?
                .value
                .parse::<u64>()
                .map_err(failure)?
                > txg
            && entry
                .properties
                .get("clones")
                .is_none_or(|p| p.value != "-" && !p.value.is_empty())
        {
            return Err(failure(format!(
                "Newer snapshot {} has dependent clones; rollback refused",
                entry.name
            )));
        }
    }
    unmount_owned(zfs, pool).await?;
    snapshot_identity(zfs, pool, source).await?;
    zfskit::dataset::rollback(
        zfs.command_runner(),
        &source.name,
        &zfskit::dataset::RollbackOptions::new().destroy_newer(),
    )
    .await
    .map_err(failure)
}

/// Persistent ordinary BE clone, independently of boot-target discovery.
pub async fn clone_environment(
    zfs: &Zfs,
    pool: &Pool,
    source: &boot::SnapshotSource,
    promote: bool,
) -> io::Result<String> {
    writable(zfs, pool).await?;
    let dataset = snapshot_identity(zfs, pool, source).await?;
    let properties = properties(zfs, &dataset).await?;
    let token = fs::read_to_string("/proc/sys/kernel/random/uuid")?
        .trim()
        .to_owned();
    if token.len() != 36 || !token.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return Err(failure("Invalid clone token"));
    }
    let target = if dataset == pool.name {
        format!("{dataset}/recovery-{token}")
    } else {
        format!("{dataset}-recovery-{token}")
    };
    let mut options = zfskit::dataset::CloneOptions::new()
        .property("mountpoint", "/")
        .property("canmount", "noauto")
        .property("readonly", "off");
    for (name, value) in [
        ("org.zfsbootmenu:commandline", properties.commandline),
        ("org.zfsbootmenu:kernel", properties.kernel),
        ("org.zfsbootmenu:rootprefix", properties.rootprefix),
    ] {
        if let Some(value) = value {
            options = options.property(name, value);
        }
    }
    options = options.property("org.zfsbootmenu:active", "on");
    zfskit::dataset::clone(zfs.command_runner(), &source.name, &target, &options)
        .await
        .map_err(failure)?;
    if promote {
        zfskit::dataset::promote(zfs.command_runner(), &target)
            .await
            .map_err(|error| {
                failure(format!(
                    "Created {target}, but promotion failed: {error}. Clone is preserved."
                ))
            })?;
    }
    Ok(target)
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

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub source: boot::SnapshotSource,
    pub creation: u64,
}

pub async fn snapshots(zfs: &Zfs, pool: &Pool, dataset: &str) -> io::Result<Vec<Snapshot>> {
    load(pool)?;
    zfskit::names::DatasetName::parse(dataset).map_err(failure)?;
    if dataset != pool.name && !dataset.starts_with(&format!("{}/", pool.name)) {
        return Err(failure("Snapshot dataset is outside selected pool"));
    }
    let entries = zfs
        .list_datasets(&zfskit::dataset::ListOptions {
            recursive: true,
            depth: Some(1),
            roots: vec![dataset.into()],
            types: vec![DatasetType::Snapshot],
            properties: vec!["name".into(), "guid".into(), "creation".into()],
        })
        .await
        .map_err(failure)?;
    let mut snapshots = vec![];
    for entry in entries {
        let name = zfskit::names::SnapshotName::parse(&entry.name).map_err(failure)?;
        if name.dataset().as_str() != dataset {
            continue;
        }
        let numeric = |property: &str| -> io::Result<u64> {
            entry
                .properties
                .get(property)
                .ok_or_else(|| failure("Missing snapshot property"))?
                .value
                .parse()
                .map_err(failure)
        };
        snapshots.push(Snapshot {
            source: boot::SnapshotSource {
                name: entry.name.clone(),
                guid: numeric("guid")?,
            },
            creation: numeric("creation")?,
        });
    }
    snapshots.sort_by(|a, b| {
        b.creation
            .cmp(&a.creation)
            .then_with(|| a.source.name.cmp(&b.source.name))
    });
    snapshots.truncate(256);
    Ok(snapshots)
}

pub async fn snapshot_targets(
    zfs: &Zfs,
    pool: &Pool,
    snapshot: &Snapshot,
    limit: u32,
) -> io::Result<boot::TargetDiscovery> {
    let source = zfskit::names::SnapshotName::parse(&snapshot.source.name).map_err(failure)?;
    let found = snapshots(zfs, pool, source.dataset().as_str())
        .await?
        .into_iter()
        .any(|s| s.source.name == snapshot.source.name && s.source.guid == snapshot.source.guid);
    if !found {
        return Err(failure("Snapshot identity changed"));
    }
    let mut lease = load(pool)?;
    let root = PathBuf::from(format!(
        "/run/zbm-rs/snapshots/{}/{}",
        pool.guid, snapshot.source.guid
    ));
    fs::create_dir_all(&root)?;
    match mount_source(&root)? {
        Some(source) if source != snapshot.source.name => {
            return Err(failure("Snapshot mount was replaced"));
        }
        Some(_) => {}
        None => {
            if !lease
                .mounts
                .iter()
                .any(|(name, path)| name == &snapshot.source.name && path == &root)
            {
                lease
                    .mounts
                    .push((snapshot.source.name.clone(), root.clone()));
                save(pool, &lease)?;
            }
            command(
                zfs,
                Cmd::new("mount")
                    .args(["-t", "zfs", "-o", "ro"])
                    .arg(&snapshot.source.name)
                    .arg(root.as_os_str()),
            )
            .await?;
        }
    }
    let properties = properties(zfs, source.dataset().as_str()).await?;
    let mut discovery = read_targets(&root, source.dataset().as_str(), limit, &properties).await?;
    for target in &mut discovery.targets {
        target.snapshot = Some(snapshot.source.clone());
    }
    Ok(discovery)
}

async fn snapshot_plan(
    target: &boot::BootTarget,
    clone: &str,
    extra_args: &[String],
) -> io::Result<boot::BootPlan> {
    let target = target.clone();
    let clone = clone.to_owned();
    let args = extra_args.to_vec();
    tokio::task::spawn_blocking(move || boot::BootPlan::for_snapshot(&target, &clone, &args))
        .await
        .map_err(failure)?
}

async fn validate_clone(zfs: &Zfs, clone: &CloneLease) -> io::Result<()> {
    use zfskit::models::PropertySourceKind;
    let dataset = zfs.dataset(&clone.dataset).map_err(failure)?;
    let origin = dataset.get_property("origin").await.map_err(failure)?;
    let owner = dataset
        .get_property("org.zbm-rs:owner")
        .await
        .map_err(failure)?;
    let state = dataset
        .get_property("org.zbm-rs:state")
        .await
        .map_err(failure)?;
    if origin.value != clone.source.name
        || owner.value != clone.token
        || owner.source.kind != PropertySourceKind::Local
        || state.value
            != if clone.handed_off {
                "retained"
            } else {
                "prepared"
            }
        || state.source.kind != PropertySourceKind::Local
    {
        return Err(failure(
            "Temporary clone ownership or origin changed; refusing reuse",
        ));
    }
    let guid = zfskit::dataset::get_property(zfs.command_runner(), &clone.source.name, "guid")
        .await
        .map_err(failure)?;
    if guid.value.parse::<u64>().map_err(failure)? != clone.source.guid {
        return Err(failure("Clone source snapshot identity changed"));
    }
    Ok(())
}

/// Explicit preparation only: never re-import writable or overwrite a dataset.
/// Journal intent before cloning; a restarted manager reuses only proven ownership.
pub async fn prepare_snapshot(
    zfs: &Zfs,
    pool: &Pool,
    target: &boot::BootTarget,
    extra_args: &[String],
) -> io::Result<boot::BootPlan> {
    let refreshed = refresh_target(zfs, target).await?;
    let target = &refreshed;
    let source = target
        .snapshot
        .as_ref()
        .ok_or_else(|| failure("Not a snapshot target"))?;
    let source_name = zfskit::names::SnapshotName::parse(&source.name).map_err(failure)?;
    if source_name.dataset().as_str() != target.dataset
        || (target.dataset != pool.name && !target.dataset.starts_with(&format!("{}/", pool.name)))
    {
        return Err(failure("Snapshot target identity mismatch"));
    }
    if !managed_pools(zfs)
        .await?
        .iter()
        .any(|p| p.guid == pool.guid && p.name == pool.name)
    {
        return Err(failure("Snapshot pool is not owned by this manager"));
    }
    let handle = zfs.pool(&pool.name).map_err(failure)?;
    if handle
        .get_property("readonly")
        .await
        .map_err(failure)?
        .value
        != "off"
    {
        return Err(failure(
            "Snapshot clone requires a writable pool import; read-only policy is preserved",
        ));
    }
    let policy = handle
        .get_property("org.zfsbootmenu:readonly")
        .await
        .map_err(failure)?
        .value;
    if policy != "-" && policy != "off" {
        return Err(failure(
            "org.zfsbootmenu:readonly forbids snapshot clone creation",
        ));
    }
    let mut lease = load(pool)?;
    let clone = if let Some(clone) = lease.clones.iter().find(|clone| {
        !clone.handed_off && clone.source.name == source.name && clone.source.guid == source.guid
    }) {
        clone.clone()
    } else {
        let token = fs::read_to_string("/proc/sys/kernel/random/uuid")?
            .trim()
            .replace('-', "");
        if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(failure("Invalid clone ownership token"));
        }
        let clone = CloneLease {
            dataset: format!("{}/zbm-rs-{token}", pool.name),
            source: source.clone(),
            token,
            handed_off: false,
        };
        // Resolve capability and all immutable boot inputs before modifying ZFS.
        snapshot_plan(target, &clone.dataset, extra_args).await?;
        lease.clones.push(clone.clone());
        save(pool, &lease)?;
        clone
    };
    let plan = snapshot_plan(target, &clone.dataset, extra_args).await?;
    let source_guid = zfskit::dataset::get_property(zfs.command_runner(), &source.name, "guid")
        .await
        .map_err(failure)?;
    if source_guid.value.parse::<u64>().map_err(failure)? != source.guid {
        return Err(failure("Snapshot identity changed before clone creation"));
    }
    let filesystems = zfs
        .list_datasets(&zfskit::dataset::ListOptions {
            recursive: true,
            roots: vec![pool.name.clone()],
            types: vec![DatasetType::Filesystem],
            ..Default::default()
        })
        .await
        .map_err(failure)?;
    if !filesystems
        .iter()
        .any(|dataset| dataset.name == clone.dataset)
    {
        let options = zfskit::dataset::CloneOptions::new()
            .property(
                "mountpoint",
                if matches!(target.backend, boot::Backend::Linux { .. }) {
                    "/"
                } else {
                    "legacy"
                },
            )
            .property("readonly", "off")
            .property("canmount", "noauto")
            .property("org.zfsbootmenu:active", "off")
            .property("org.zbm-rs:owner", &clone.token)
            .property("org.zbm-rs:state", "prepared");
        if let Err(error) =
            zfskit::dataset::clone(zfs.command_runner(), &source.name, &clone.dataset, &options)
                .await
        {
            // Failed creation is never evidence of ownership of a collision.
            lease.clones.retain(|item| item.token != clone.token);
            save(pool, &lease)?;
            return Err(failure(error));
        }
    }
    validate_clone(zfs, &clone).await?;
    Ok(plan)
}

/// Keep any booted clone: its writable state belongs to the target OS/user.
pub async fn retain_snapshot_clone(
    zfs: &Zfs,
    pool: &Pool,
    plan: &boot::BootPlan,
) -> io::Result<()> {
    if plan.target.snapshot.is_none() {
        return Ok(());
    }
    let mut lease = load(pool)?;
    let clone = lease
        .clones
        .iter_mut()
        .find(|clone| clone.dataset == plan.target.dataset)
        .ok_or_else(|| failure("Missing temporary clone journal"))?;
    validate_clone(zfs, clone).await?;
    zfs.dataset(&clone.dataset)
        .map_err(failure)?
        .set_property("org.zbm-rs:state", "retained")
        .await
        .map_err(failure)?;
    clone.handed_off = true;
    save(pool, &lease)
}

/// An explicit user action; never destroy retained clones, children, or snapshots.
pub async fn discard_prepared_clone(
    zfs: &Zfs,
    pool: &Pool,
    source: &boot::SnapshotSource,
) -> io::Result<String> {
    writable(zfs, pool).await?;
    if !managed_pools(zfs)
        .await?
        .iter()
        .any(|p| p.guid == pool.guid && p.name == pool.name)
    {
        return Err(failure("Snapshot pool is not owned by this manager"));
    }
    let mut lease = load(pool)?;
    let clone = lease
        .clones
        .iter()
        .find(|clone| {
            !clone.handed_off
                && clone.source.name == source.name
                && clone.source.guid == source.guid
        })
        .ok_or_else(|| failure("No prepared clone for this snapshot"))?
        .clone();
    validate_clone(zfs, &clone).await?;
    if fs::read_to_string("/proc/self/mountinfo")?
        .lines()
        .any(|line| {
            line.split_once(" - ").is_some_and(|(_, after)| {
                after.split_whitespace().nth(1) == Some(clone.dataset.as_str())
            })
        })
    {
        return Err(failure("Prepared clone has a mount; refusing destruction"));
    }
    zfs.dataset(&clone.dataset)
        .map_err(failure)?
        .destroy(&Default::default())
        .await
        .map_err(failure)?;
    lease.clones.retain(|record| record.token != clone.token);
    save(pool, &lease)?;
    Ok(clone.dataset)
}
