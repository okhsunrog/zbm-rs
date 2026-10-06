//! Bootspec resolution is independent of rendering and execution.
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs, io,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Serialize)]
pub struct BootTarget {
    pub dataset: String,
    pub generation: Option<u64>,
    pub label: String,
    pub root: PathBuf,
    /// Guest-absolute path, not a host path under the loader's /nix/store.
    pub toplevel: Option<PathBuf>,
    pub backend: Backend,
    pub snapshot: Option<SnapshotSource>,
    pub mountpoint: Option<String>,
    pub issue: Option<String>,
    /// Guest paths and arguments for inspection. Execution revalidates Bootspec.
    pub inputs: BootInputs,
}

#[derive(Debug, Clone, Serialize)]
pub struct BootInputs {
    pub kernel: PathBuf,
    pub initrd: Option<PathBuf>,
    pub init: Option<PathBuf>,
    pub kernel_params: Vec<String>,
}

impl BootInputs {
    fn from_spec(spec: &Bootspec) -> Self {
        Self {
            kernel: spec.kernel.clone(),
            initrd: spec.initrd.clone(),
            init: Some(spec.init.clone()),
            kernel_params: spec.kernel_params.clone(),
        }
    }

    pub fn command_line(&self, extra_args: &[String]) -> Vec<String> {
        let mut args: Vec<String> = self
            .init
            .iter()
            .map(|init| format!("init={}", init.display()))
            .collect();
        args.extend(self.kernel_params.iter().cloned());
        args.extend_from_slice(extra_args);
        args
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum Backend {
    Nixos,
    Linux {
        rootprefix: String,
        commandline: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotSource {
    pub name: String,
    pub guid: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GenerationIssue {
    pub generation: u64,
    pub error: String,
}

#[derive(Debug, Default)]
pub struct TargetDiscovery {
    pub targets: Vec<BootTarget>,
    pub rejected: Vec<GenerationIssue>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BootPlan {
    pub target: BootTarget,
    pub kernel: PathBuf,
    pub initrd: Option<PathBuf>,
    pub cmdline: Vec<String>,
}

#[derive(Deserialize)]
struct Document {
    #[serde(rename = "org.nixos.bootspec.v1")]
    boot: Bootspec,
    #[serde(rename = "org.nixos.extra-initrd.v1")]
    extra_initrd: Option<serde_json::Value>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bootspec {
    system: String,
    kernel: PathBuf,
    initrd: Option<PathBuf>,
    initrd_secrets: Option<String>,
    init: PathBuf,
    toplevel: PathBuf,
    kernel_params: Vec<String>,
    label: String,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Resolve absolute symlinks relative to the mounted OS root, never host /.
/// Reject root escape and cap symlink traversal (including cyclic profiles).
pub fn rooted_path(root: &Path, path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(invalid("Boot path must be absolute"));
    }
    let mut pending: VecDeque<_> = path
        .components()
        .map(|c| c.as_os_str().to_owned())
        .collect();
    let mut relative = PathBuf::new();
    let mut links = 0;
    while let Some(part) = pending.pop_front() {
        match Path::new(&part).components().next().unwrap() {
            Component::RootDir => relative.clear(),
            Component::CurDir => {}
            Component::ParentDir => {
                if !relative.pop() {
                    return Err(invalid("Boot path escapes OS root"));
                }
            }
            Component::Normal(_) => {
                relative.push(&part);
                let candidate = root.join(&relative);
                if fs::symlink_metadata(&candidate)?.file_type().is_symlink() {
                    links += 1;
                    if links > 40 {
                        return Err(invalid("Too many boot path symlinks"));
                    }
                    let target = fs::read_link(candidate)?;
                    relative.pop();
                    for component in target.components().rev() {
                        pending.push_front(component.as_os_str().to_owned());
                    }
                }
            }
            _ => return Err(invalid("Unsupported boot path component")),
        }
    }
    Ok(root.join(relative))
}

fn read_document(root: &Path, toplevel: &Path) -> io::Result<Document> {
    let path = rooted_path(root, &toplevel.join("boot.json"))?;
    let metadata = fs::metadata(&path)?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err(invalid("Bootspec exceeds 1 MiB"));
    }
    let document: Document = serde_json::from_slice(&fs::read(path)?).map_err(invalid_error)?;
    if document.extra_initrd.is_some() {
        return Err(invalid(
            "Bootspec extra initrd extension is not supported yet",
        ));
    }
    let system = match std::env::consts::ARCH {
        "x86_64" => "x86_64-linux",
        "aarch64" => "aarch64-linux",
        _ => return Err(invalid("Unsupported loader architecture")),
    };
    if document.boot.system != system {
        return Err(invalid("Unsupported Bootspec architecture"));
    }
    if document.boot.initrd_secrets.is_some() {
        return Err(invalid("Bootspec initrdSecrets is not supported yet"));
    }
    Ok(document)
}
fn read_spec(root: &Path, toplevel: &Path) -> io::Result<Bootspec> {
    Ok(read_document(root, toplevel)?.boot)
}
pub fn supports_snapshot(target: &BootTarget) -> io::Result<bool> {
    if matches!(target.backend, Backend::Linux { .. }) {
        return Ok(true);
    }
    let spec = read_spec(
        &target.root,
        target
            .toplevel
            .as_ref()
            .ok_or_else(|| invalid("Missing NixOS toplevel"))?,
    )?;
    let initrd = spec
        .initrd
        .ok_or_else(|| invalid("NixOS clone boot requires an initrd"))?;
    crate::initrd::systemd(&rooted_path(&target.root, &initrd)?)
}
fn invalid_error(error: impl std::fmt::Display) -> io::Error {
    invalid(error.to_string())
}

pub fn generations(root: &Path, dataset: &str, limit: u32) -> io::Result<Vec<BootTarget>> {
    Ok(discover_generations(root, dataset, limit)?.targets)
}

pub fn discover_generations(root: &Path, dataset: &str, limit: u32) -> io::Result<TargetDiscovery> {
    let directory = match rooted_path(root, Path::new("/nix/var/nix/profiles")) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(TargetDiscovery::default());
        }
        Err(error) => return Err(error),
    };
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(TargetDiscovery::default());
        }
        Err(error) => return Err(error),
    };
    let mut candidates = vec![];
    for entry in entries {
        let name = entry?.file_name();
        if let Some(number) = name
            .to_str()
            .and_then(|s| s.strip_prefix("system-"))
            .and_then(|s| s.strip_suffix("-link"))
            .and_then(|s| s.parse::<u64>().ok())
        {
            candidates.push(number);
        }
    }
    candidates.sort_unstable_by(|a, b| b.cmp(a));
    candidates.truncate(limit as usize);
    let mut discovery = TargetDiscovery::default();
    for generation in candidates {
        let profile = PathBuf::from(format!("/nix/var/nix/profiles/system-{generation}-link"));
        let target = (|| {
            let spec = read_spec(root, &profile)?;
            let actual = rooted_path(root, &profile)?;
            if actual != rooted_path(root, &spec.toplevel)? {
                return Err(invalid("Bootspec toplevel does not match generation"));
            }
            Ok(BootTarget {
                inputs: BootInputs::from_spec(&spec),
                dataset: dataset.into(),
                generation: Some(generation),
                label: spec.label,
                root: root.into(),
                toplevel: Some(spec.toplevel),
                backend: Backend::Nixos,
                snapshot: None,
                mountpoint: None,
                issue: None,
            })
        })();
        match target {
            Ok(target) => discovery.targets.push(target),
            Err(error) => discovery.rejected.push(GenerationIssue {
                generation,
                error: error.to_string(),
            }),
        }
    }
    Ok(discovery)
}

impl BootPlan {
    pub fn resolve(target: &BootTarget, extra_args: &[String]) -> io::Result<Self> {
        if matches!(target.backend, Backend::Linux { .. }) {
            return crate::linux::resolve(target, &target.dataset, extra_args);
        }
        Self::resolve_nixos(target, extra_args, None)
    }

    fn resolve_nixos(
        target: &BootTarget,
        extra_args: &[String],
        root_override: Option<&str>,
    ) -> io::Result<Self> {
        let toplevel = target
            .toplevel
            .as_ref()
            .ok_or_else(|| invalid("Missing NixOS toplevel"))?;
        let spec = read_spec(&target.root, toplevel)?;
        let fstab_path = rooted_path(&target.root, &spec.toplevel.join("etc/fstab"))?;
        let metadata = fs::metadata(&fstab_path)?;
        if !metadata.is_file() || metadata.len() > 1024 * 1024 {
            return Err(invalid("Invalid target fstab"));
        }
        let fstab = fs::read_to_string(fstab_path)?;
        let mounts: Vec<Vec<&str>> = fstab
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .map(|line| line.split_whitespace().collect())
            .filter(|fields: &Vec<_>| fields.len() >= 3)
            .collect();
        let roots: Vec<_> = mounts.iter().filter(|fields| fields[1] == "/").collect();
        if roots.len() != 1 || roots[0][2] != "zfs" {
            return Err(invalid(
                "NixOS configuration root does not match selected ZFS dataset",
            ));
        }
        if roots[0][0] != target.dataset && root_override.is_none() {
            return Self::resolve_nixos(target, extra_args, Some(&target.dataset));
        }
        if mounts
            .iter()
            .any(|fields| fields[1] == "/nix" || fields[1].starts_with("/nix/"))
        {
            return Err(invalid("Separate Nix store mounts are not supported yet"));
        }
        let cmdline = BootInputs::from_spec(&spec).command_line(extra_args);
        for argument in &cmdline {
            if argument.is_empty()
                || argument
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\'')
            {
                return Err(invalid("Unsupported whitespace/quoting in kernel argument"));
            }
        }
        if extra_args
            .iter()
            .any(|a| a.starts_with("init=") || a.starts_with("root="))
        {
            return Err(invalid("Cannot override Bootspec init or root"));
        }
        if cmdline
            .iter()
            .any(|a| a.starts_with("root=") && a != "root=fstab")
        {
            return Err(invalid("Only the configured NixOS fstab root is supported"));
        }
        rooted_path(&target.root, &spec.init)?;
        let plan = Self {
            target: target.clone(),
            kernel: rooted_path(&target.root, &spec.kernel)?,
            initrd: spec
                .initrd
                .map(|p| rooted_path(&target.root, &p))
                .transpose()?,
            cmdline,
        };
        for path in std::iter::once(&plan.kernel).chain(plan.initrd.iter()) {
            if !fs::metadata(path)?.is_file() {
                return Err(invalid("Kernel and initrd must be regular files"));
            }
        }
        if let Some(root) = root_override {
            if !supports_snapshot(target)? {
                return Err(invalid("NixOS root override requires systemd initrd"));
            }
            Self::rewrite_systemd_root(plan, target, root)
        } else {
            Ok(plan)
        }
    }

    /// The caller has validated and created this clone from the immutable source.
    pub fn for_snapshot(
        target: &BootTarget,
        clone: &str,
        extra_args: &[String],
    ) -> io::Result<Self> {
        zfskit::names::DatasetName::parse(clone).map_err(invalid_error)?;
        if target.snapshot.is_none() || !supports_snapshot(target)? {
            return Err(invalid(
                "Cannot boot this NixOS initrd on a clone: systemd initrd is required",
            ));
        }
        if matches!(target.backend, Backend::Linux { .. }) {
            let mut plan = crate::linux::resolve(target, clone, extra_args)?;
            plan.target.dataset = clone.into();
            return Ok(plan);
        }
        Self::resolve_nixos(target, extra_args, Some(clone))
    }

    fn rewrite_systemd_root(mut plan: Self, target: &BootTarget, clone: &str) -> io::Result<Self> {
        zfskit::names::DatasetName::parse(clone).map_err(invalid_error)?;
        let fstab_path = rooted_path(
            &target.root,
            &target.toplevel.as_ref().unwrap().join("etc/fstab"),
        )?;
        if fs::read_to_string(fstab_path)?
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .map(|line| line.split_whitespace().collect::<Vec<_>>())
            .any(|fields| {
                fields.len() >= 3
                    && fields[1] != "/"
                    && (fields[1] == "/usr"
                        || fields.get(3).is_some_and(|options| {
                            options.split(',').any(|option| option == "x-initrd.mount")
                        }))
            })
        {
            return Err(invalid(
                "Snapshot boot does not support additional initrd mounts",
            ));
        }
        if plan.cmdline.iter().any(|arg| {
            arg.starts_with("rootflags=")
                || arg.starts_with("rootfstype=")
                || arg.starts_with("rd.fstab=")
                || arg == "ro"
        }) {
            return Err(invalid(
                "Snapshot boot cannot override conflicting root policy",
            ));
        }
        plan.cmdline.retain(|arg| !arg.starts_with("root="));
        plan.cmdline.extend([
            format!("root={clone}"),
            "rootfstype=zfs".into(),
            if clone == target.dataset
                && target.mountpoint.as_deref().is_some_and(|p| p != "legacy")
            {
                "rootflags=defaults,zfsutil".into()
            } else {
                "rootflags=defaults".into()
            },
            "rw".into(),
            "rd.fstab=no".into(),
        ]);
        plan.target.dataset = clone.into();
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guest_absolute_symlinks_never_resolve_against_loader_root() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("zbm-path-test-{}", std::process::id()));
        fs::create_dir_all(root.join("nix/store/os")).unwrap();
        fs::write(root.join("nix/store/os/kernel"), b"fixture").unwrap();
        symlink("/nix/store/os/kernel", root.join("kernel")).unwrap();
        symlink("../../outside", root.join("escape")).unwrap();
        symlink("/cycle", root.join("cycle")).unwrap();
        assert_eq!(
            rooted_path(&root, Path::new("/kernel")).unwrap(),
            root.join("nix/store/os/kernel")
        );
        assert!(rooted_path(&root, Path::new("/escape")).is_err());
        assert!(rooted_path(&root, Path::new("/cycle")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn bootspec_selects_generation_and_rejects_unsupported_boot_inputs() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("zbm-bootspec-test-{}", std::process::id()));
        fs::create_dir_all(root.join("nix/var/nix/profiles")).unwrap();
        fs::create_dir_all(root.join("nix/store/os")).unwrap();
        fs::create_dir_all(root.join("nix/store/os/etc")).unwrap();
        fs::write(
            root.join("nix/store/os/etc/fstab"),
            "tank/root / zfs defaults 0 0\n",
        )
        .unwrap();
        for name in ["kernel", "initrd", "init"] {
            fs::write(root.join("nix/store/os").join(name), b"fixture").unwrap();
        }
        let mut document = serde_json::json!({"org.nixos.bootspec.v1":{
            "system":"x86_64-linux", "kernel":"/nix/store/os/kernel", "initrd":"/nix/store/os/initrd",
            "init":"/nix/store/os/init", "toplevel":"/nix/store/os", "kernelParams":["root=fstab"], "label":"fixture"
        }});
        let path = root.join("nix/store/os/boot.json");
        fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        for number in [3, 12] {
            symlink(
                "/nix/store/os",
                root.join(format!("nix/var/nix/profiles/system-{number}-link")),
            )
            .unwrap();
        }
        let targets = generations(&root, "tank/root", 1).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].generation, Some(12));
        let plan = BootPlan::resolve(&targets[0], &["quiet".into()]).unwrap();
        assert_eq!(plan.kernel, root.join("nix/store/os/kernel"));
        assert_eq!(
            plan.cmdline,
            ["init=/nix/store/os/init", "root=fstab", "quiet"]
        );
        assert!(BootPlan::resolve(&targets[0], &["init=/wrong".into()]).is_err());
        assert!(BootPlan::resolve(&targets[0], &["two words".into()]).is_err());
        let mut wrong_root = targets[0].clone();
        wrong_root.dataset = "tank/other-root".into();
        assert!(BootPlan::resolve(&wrong_root, &[]).is_err());
        let mut snapshot = targets[0].clone();
        snapshot.snapshot = Some(SnapshotSource {
            name: "tank/root@good".into(),
            guid: 9,
        });
        assert!(BootPlan::for_snapshot(&snapshot, "tank/clone", &[]).is_err());
        fs::write(
            root.join("nix/store/os/initrd"),
            crate::initrd::tests::archive("/usr/lib/systemd/systemd"),
        )
        .unwrap();
        let clone_plan = BootPlan::for_snapshot(&snapshot, "tank/clone", &[]).unwrap();
        assert!(clone_plan.cmdline.contains(&"root=tank/clone".into()));
        assert!(clone_plan.cmdline.contains(&"rootfstype=zfs".into()));
        assert!(!clone_plan.cmdline.contains(&"root=fstab".into()));
        assert_eq!(clone_plan.target.dataset, "tank/clone");
        assert_eq!(clone_plan.kernel, plan.kernel);
        let mut persistent = targets[0].clone();
        persistent.dataset = "tank/promoted".into();
        persistent.mountpoint = Some("/".into());
        let persistent_plan = BootPlan::resolve(&persistent, &[]).unwrap();
        assert!(
            persistent_plan
                .cmdline
                .contains(&"root=tank/promoted".into())
        );
        assert!(
            persistent_plan
                .cmdline
                .contains(&"rootflags=defaults,zfsutil".into())
        );
        assert!(persistent_plan.target.snapshot.is_none());
        assert!(BootPlan::for_snapshot(&snapshot, "tank/clone", &["rootflags=ro".into()]).is_err());
        document["org.nixos.bootspec.v1"]["initrdSecrets"] = "/nix/store/os/secret-appender".into();
        fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(BootPlan::resolve(&targets[0], &[]).is_err());
        document["org.nixos.bootspec.v1"]
            .as_object_mut()
            .unwrap()
            .remove("initrdSecrets");
        document["org.nixos.bootspec.v1"]["system"] = "aarch64-linux".into();
        fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        let discovery = discover_generations(&root, "tank/root", 20).unwrap();
        assert!(discovery.targets.is_empty());
        assert_eq!(discovery.rejected.len(), 2);
        assert!(discovery.rejected[0].error.contains("architecture"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn broken_and_unsupported_generations_do_not_hide_usable_generations() {
        use std::os::unix::fs::symlink;
        let root =
            std::env::temp_dir().join(format!("zbm-mixed-generations-{}", std::process::id()));
        fs::create_dir_all(root.join("nix/var/nix/profiles")).unwrap();
        for number in 1..=4 {
            let toplevel = format!("/nix/store/os-{number}");
            let directory = root.join(toplevel.trim_start_matches('/'));
            fs::create_dir_all(&directory).unwrap();
            let mut document = serde_json::json!({"org.nixos.bootspec.v1": {
                "system": "x86_64-linux", "kernel": format!("{toplevel}/kernel"),
                "init": format!("{toplevel}/init"), "toplevel": toplevel,
                "kernelParams": [], "label": format!("generation {number}")
            }});
            if number == 2 {
                document["org.nixos.bootspec.v1"]["initrdSecrets"] = "/unsupported".into();
            }
            fs::write(
                directory.join("boot.json"),
                if number == 3 {
                    b"broken-json".to_vec()
                } else {
                    serde_json::to_vec(&document).unwrap()
                },
            )
            .unwrap();
            symlink(
                &toplevel,
                root.join(format!("nix/var/nix/profiles/system-{number}-link")),
            )
            .unwrap();
        }
        let discovery = discover_generations(&root, "tank/root", 4).unwrap();
        assert_eq!(
            discovery
                .targets
                .iter()
                .map(|target| target.generation)
                .collect::<Vec<_>>(),
            [Some(4), Some(1)]
        );
        assert_eq!(
            discovery
                .rejected
                .iter()
                .map(|issue| issue.generation)
                .collect::<Vec<_>>(),
            [3, 2]
        );
        assert!(discovery.rejected[1].error.contains("initrdSecrets"));
        fs::remove_dir_all(root).unwrap();
    }
}
