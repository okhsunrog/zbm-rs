//! Bootspec resolution is independent of rendering and execution.
pub(crate) mod planning;
mod record;
use record::TargetRecord;

use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs, io,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "TargetRecord", into = "TargetRecord")]
pub struct BootTarget {
    pub dataset: String,
    pub label: String,
    pub root: PathBuf,
    pub backend: Backend,
    pub snapshot: Option<SnapshotSource>,
    pub mountpoint: Option<String>,
}

impl BootTarget {
    pub fn generation(&self) -> Option<u64> {
        match &self.backend {
            Backend::Nixos(t) => Some(t.generation),
            Backend::Linux(_) => None,
        }
    }
    pub fn toplevel(&self) -> Option<&Path> {
        match &self.backend {
            Backend::Nixos(t) => Some(&t.toplevel),
            Backend::Linux(_) => None,
        }
    }
    pub fn issue(&self) -> Option<&str> {
        match &self.backend {
            Backend::Nixos(_) => None,
            Backend::Linux(t) => t.issue.as_deref(),
        }
    }
    /// Discovery data for display only. Execution revalidates the inputs.
    pub fn inputs(&self) -> BootInputs {
        match &self.backend {
            Backend::Nixos(t) => BootInputs {
                kernel: t.kernel.clone(),
                initrd: t.initrd.clone(),
                init: Some(t.init.clone()),
                kernel_params: t.kernel_params.clone(),
            },
            Backend::Linux(t) => BootInputs {
                kernel: t.kernel.clone(),
                initrd: t.initrd.clone(),
                init: None,
                kernel_params: t.commandline.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
        self.init
            .iter()
            .map(|init| format!("init={}", init.display()))
            .chain(self.kernel_params.iter().cloned())
            .chain(extra_args.iter().cloned())
            .collect()
    }
}

#[derive(Debug, Clone)]
pub enum Backend {
    Nixos(NixosTarget),
    Linux(LinuxTarget),
}

#[derive(Debug, Clone)]
pub struct NixosTarget {
    pub generation: u64,
    pub toplevel: PathBuf,
    pub init: PathBuf,
    pub kernel: PathBuf,
    pub initrd: Option<PathBuf>,
    pub kernel_params: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LinuxTarget {
    pub kernel: PathBuf,
    pub initrd: Option<PathBuf>,
    pub rootprefix: String,
    pub commandline: Vec<String>,
    pub issue: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotSource {
    pub name: String,
    pub guid: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationIssue {
    pub generation: u64,
    pub error: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TargetDiscovery {
    pub targets: Vec<BootTarget>,
    pub rejected: Vec<GenerationIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    if matches!(target.backend, Backend::Linux(_)) {
        return Ok(true);
    }
    let spec = read_spec(
        &target.root,
        target
            .toplevel()
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
                dataset: dataset.into(),
                label: spec.label,
                root: root.into(),
                backend: Backend::Nixos(NixosTarget {
                    generation,
                    toplevel: spec.toplevel,
                    init: spec.init,
                    kernel: spec.kernel,
                    initrd: spec.initrd,
                    kernel_params: spec.kernel_params,
                }),
                snapshot: None,
                mountpoint: None,
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
    /// A verified NixOS target needs an explicit typed ZFS root instead of an
    /// indirect fstab selector. Existing initrd checks forbid unsupported rewrites.
    pub fn resolve_verified(target: &BootTarget, extra_args: &[String]) -> io::Result<Self> {
        if matches!(target.backend, Backend::Linux(_)) {
            Self::resolve(target, extra_args)
        } else {
            Self::resolve_nixos(target, extra_args, Some(&target.dataset))
        }
    }

    pub fn resolve(target: &BootTarget, extra_args: &[String]) -> io::Result<Self> {
        if matches!(target.backend, Backend::Linux(_)) {
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
            .toplevel()
            .ok_or_else(|| invalid("Missing NixOS toplevel"))?;
        let spec = read_spec(&target.root, toplevel)?;
        let fstab_path = rooted_path(&target.root, &spec.toplevel.join("etc/fstab"))?;
        let metadata = fs::metadata(&fstab_path)?;
        if !metadata.is_file() || metadata.len() > 1024 * 1024 {
            return Err(invalid("Invalid target fstab"));
        }
        let fstab = fs::read_to_string(fstab_path)?;
        let checked = planning::NixosConfiguration::new(spec, fstab).check(
            target.clone(),
            extra_args,
            root_override,
        )?;
        let systemd_initrd = checked
            .required_initrd()?
            .map(|path| planning::SystemdInitrd::inspect(&target.root, path))
            .transpose()?;
        checked.plan(systemd_initrd.as_ref())?.resolve()
    }

    /// The caller has validated and created this clone from the immutable source.
    pub fn for_snapshot(
        target: &BootTarget,
        clone: &str,
        extra_args: &[String],
    ) -> io::Result<Self> {
        zfskit::names::DatasetName::parse(clone).map_err(invalid_error)?;
        if target.snapshot.is_none() {
            return Err(invalid(
                "Cannot boot this NixOS initrd on a clone: systemd initrd is required",
            ));
        }
        if matches!(target.backend, Backend::Linux(_)) {
            let mut plan = crate::linux::resolve(target, clone, extra_args)?;
            plan.target.dataset = clone.into();
            return Ok(plan);
        }
        Self::resolve_nixos(target, extra_args, Some(clone))
    }
}

impl planning::GuestPlan {
    /// Effect boundary: resolve guest symlinks and check actual boot files.
    pub(crate) fn resolve(self) -> io::Result<BootPlan> {
        if let Some(init) = &self.inputs.init {
            init.resolve(&self.target.root)?;
        }
        let kernel = self.inputs.kernel.resolve(&self.target.root)?;
        let initrd = self
            .inputs
            .initrd
            .as_ref()
            .map(|path| path.resolve(&self.target.root))
            .transpose()?;
        for path in std::iter::once(&kernel).chain(initrd.iter()) {
            if !fs::metadata(&path.0)?.is_file() {
                return Err(invalid("Kernel and initrd must be regular files"));
            }
        }
        Ok(BootPlan {
            target: self.target,
            kernel: kernel.0,
            initrd: initrd.map(|path| path.0),
            cmdline: self.cmdline,
        })
    }
}

impl planning::GuestPath {
    fn resolve(&self, root: &Path) -> io::Result<planning::ResolvedPath> {
        rooted_path(root, self.as_path()).map(planning::ResolvedPath)
    }
}

impl planning::SystemdInitrd {
    fn inspect(root: &Path, path: &Path) -> io::Result<Self> {
        let path = planning::GuestPath::new(path.into())?;
        if !crate::initrd::systemd(&path.resolve(root)?.0)? {
            return Err(invalid("NixOS root override requires systemd initrd"));
        }
        Ok(Self {
            root: root.into(),
            path,
        })
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
        assert_eq!(targets[0].generation(), Some(12));
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
                .map(|target| target.generation())
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
