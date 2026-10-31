//! Bootspec resolution is independent of rendering and execution.
pub(crate) mod planning;
mod record;
use record::TargetRecord;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
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
    pub fn specialisation(&self) -> Option<&str> {
        match &self.backend {
            Backend::Nixos(t) => t.specialisation.as_deref(),
            Backend::Linux(_) => None,
        }
    }
    /// Catalog identity, including variants sharing a kernel within one generation.
    pub fn same_identity(&self, other: &Self) -> bool {
        self.dataset == other.dataset
            && self.generation() == other.generation()
            && self.specialisation() == other.specialisation()
            && self.toplevel() == other.toplevel()
            && self.inputs().kernel == other.inputs().kernel
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

/// Prefer a named variant of the discovered default's generation only. Never
/// substitute an older generation or invent a default for an ambiguous catalog.
pub fn preferred_specialisation(
    targets: &[BootTarget],
    default: Option<usize>,
    preferred: Option<&str>,
) -> Option<usize> {
    let Some(base) = default.and_then(|index| targets.get(index)) else {
        return default;
    };
    let (Some(generation), Some(name)) = (base.generation(), preferred) else {
        return default;
    };
    targets
        .iter()
        .position(|target| {
            target.dataset == base.dataset
                && target.generation() == Some(generation)
                && target.specialisation() == Some(name)
        })
        .or(default)
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
    pub specialisation: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specialisation: Option<String>,
    pub error: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TargetDiscovery {
    pub targets: Vec<BootTarget>,
    pub rejected: Vec<GenerationIssue>,
    pub default_target: Option<usize>,
    pub default_error: Option<String>,
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
    #[serde(rename = "org.nixos.specialisation.v1", default)]
    specialisations: BTreeMap<String, serde_json::Value>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bootspec {
    #[serde(skip)]
    prepared: bool,
    system: String,
    kernel: PathBuf,
    initrd: Option<PathBuf>,
    initrd_secrets: Option<String>,
    init: PathBuf,
    toplevel: PathBuf,
    kernel_params: Vec<String>,
    label: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedGeneration {
    version: u32,
    bootspec_sha256: String,
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
                if fs::symlink_metadata(&candidate)
                    .map_err(|error| {
                        io::Error::new(
                            error.kind(),
                            format!("Boot path {}: {error}", candidate.display()),
                        )
                    })?
                    .file_type()
                    .is_symlink()
                {
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

fn source_document(root: &Path, toplevel: &Path) -> io::Result<Vec<u8>> {
    let path = rooted_path(root, &toplevel.join("boot.json"))?;
    let metadata = fs::metadata(&path)?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err(invalid("Bootspec exceeds 1 MiB"));
    }
    fs::read(path)
}

fn read_document(root: &Path, toplevel: &Path) -> io::Result<Document> {
    let bytes = source_document(root, toplevel)?;
    let mut document: Document = serde_json::from_slice(&bytes).map_err(invalid_error)?;
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
    // Source Bootspec stays immutable. Installation prepares copies outside the
    // store; neither discovery nor the boot broker executes initrdSecrets.
    let canonical = rooted_path(root, &document.boot.toplevel)?;
    let guest = canonical.strip_prefix(root).map_err(invalid_error)?;
    if let Ok(name) = guest.strip_prefix("nix/store")
        && name.components().count() == 1
    {
        let directory = Path::new("/boot/zbm-rs/nixos").join(name);
        match rooted_path(root, &directory.join("prepared.json")) {
            Ok(record) => {
                if !fs::metadata(&record)?.is_file() || fs::metadata(&record)?.len() > 4096 {
                    return Err(invalid("Invalid prepared NixOS generation record"));
                }
                let prepared: PreparedGeneration =
                    serde_json::from_slice(&fs::read(record)?).map_err(invalid_error)?;
                if prepared.version != 1
                    || prepared.bootspec_sha256 != crate::security::hex(&Sha256::digest(&bytes))
                {
                    return Err(invalid("Prepared NixOS generation does not match Bootspec"));
                }
                document.boot.kernel = directory.join("kernel");
                if document.boot.initrd.is_some() {
                    document.boot.initrd = Some(directory.join("initrd"));
                }
                document.boot.initrd_secrets = None;
                document.boot.prepared = true;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    if document.boot.initrd_secrets.is_some() {
        return Err(invalid(
            "NixOS initrdSecrets requires an installed prepared generation",
        ));
    }
    document.boot.kernel_params = document
        .boot
        .kernel_params
        .iter()
        .map(|parameter| {
            let mut tokens = crate::linux::arguments(parameter)?;
            if tokens.len() != 1 {
                return Err(invalid(
                    "Bootspec parameter must contain one quoted kernel argument",
                ));
            }
            Ok(tokens.remove(0))
        })
        .collect::<io::Result<_>>()?;
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

/// Installation and discovery use the same Bootspec/prepared-input boundary.
pub fn nixos_target(
    root: &Path,
    dataset: &str,
    profile: &Path,
    generation: u64,
) -> io::Result<BootTarget> {
    let spec = read_spec(root, profile)?;
    if rooted_path(root, profile)? != rooted_path(root, &spec.toplevel)? {
        return Err(invalid("Bootspec toplevel does not match generation"));
    }
    Ok(BootTarget {
        dataset: dataset.into(),
        label: spec.label,
        root: root.into(),
        backend: Backend::Nixos(NixosTarget {
            generation,
            specialisation: None,
            toplevel: spec.toplevel,
            init: spec.init,
            kernel: spec.kernel,
            initrd: spec.initrd,
            kernel_params: spec.kernel_params,
        }),
        snapshot: None,
        mountpoint: None,
    })
}

fn specialisation_target(
    root: &Path,
    dataset: &str,
    profile: &Path,
    generation: u64,
    name: &str,
    declared: &serde_json::Value,
) -> io::Result<BootTarget> {
    if name.contains('/')
        || name.is_empty()
        || name.chars().any(|c| c.is_control())
        || Path::new(name).components().count() != 1
        || !matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        )
    {
        return Err(invalid("Invalid specialisation name"));
    }
    let child = profile.join("specialisation").join(name);
    let actual: serde_json::Value =
        serde_json::from_slice(&source_document(root, &child)?).map_err(invalid_error)?;
    if actual != *declared {
        return Err(invalid(
            "Specialisation Bootspec does not match parent declaration",
        ));
    }
    let mut target = nixos_target(root, dataset, &child, generation)?;
    if let Backend::Nixos(ref mut nixos) = target.backend {
        nixos.specialisation = Some(name.into());
    }
    Ok(target)
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
    let current = rooted_path(root, Path::new("/nix/var/nix/profiles/system"));
    // A rollback may select an old generation outside the normal history limit.
    let profile_number = fs::read_link(root.join("nix/var/nix/profiles/system"))
        .ok()
        .and_then(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("system-"))
                .and_then(|name| name.strip_suffix("-link"))
                .and_then(|name| name.parse::<u64>().ok())
        });
    let current_number = profile_number.or_else(|| {
        candidates.iter().copied().find(|number| {
            rooted_path(
                root,
                &PathBuf::from(format!("/nix/var/nix/profiles/system-{number}-link")),
            )
            .ok()
            .as_ref()
            .is_some_and(|path| current.as_ref().ok() == Some(path))
        })
    });
    candidates.truncate(limit as usize);
    if let Some(number) = current_number
        && !candidates.contains(&number)
    {
        candidates.pop();
        candidates.push(number);
        candidates.sort_unstable_by(|a, b| b.cmp(a));
    }
    let mut discovery = TargetDiscovery::default();
    for generation in candidates {
        let profile = PathBuf::from(format!("/nix/var/nix/profiles/system-{generation}-link"));
        let mut add = |variant: Option<String>, result: io::Result<BootTarget>| match result {
            Ok(target) => discovery.targets.push(target),
            Err(error) => discovery.rejected.push(GenerationIssue {
                generation,
                specialisation: variant,
                error: error.to_string(),
            }),
        };
        add(None, nixos_target(root, dataset, &profile, generation));
        // A broken child cannot hide the parent or its usable siblings.
        if let Ok(bytes) = source_document(root, &profile)
            && let Ok(document) = serde_json::from_slice::<Document>(&bytes)
        {
            for (name, declared) in document.specialisations {
                add(
                    Some(name.clone()),
                    specialisation_target(root, dataset, &profile, generation, &name, &declared),
                );
            }
        }
    }

    discovery.default_target = current_number
        .and_then(|number| {
            discovery.targets.iter().position(|target| {
                target.generation() == Some(number) && target.specialisation().is_none()
            })
        })
        .or_else(|| {
            if profile_number.is_some() {
                None
            } else {
                current.as_ref().ok().and_then(|current| {
                    discovery.targets.iter().position(|target| {
                        target
                            .toplevel()
                            .and_then(|path| rooted_path(root, path).ok())
                            .as_ref()
                            == Some(current)
                    })
                })
            }
        });
    if discovery.default_target.is_none() {
        discovery.default_error = Some(
            "Current NixOS system profile is missing or not prepared; choose a generation with K"
                .into(),
        );
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
        let (systemd, root_generator) = crate::initrd::features(&path.resolve(root)?.0)?;
        if !systemd {
            return Err(invalid("NixOS root override requires systemd initrd"));
        }
        Ok(Self {
            root: root.into(),
            path,
            root_generator,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preference_stays_in_the_default_generation_and_dataset() {
        let target = |generation, specialisation: Option<&str>, dataset: &str| BootTarget {
            dataset: dataset.into(),
            label: "fixture".into(),
            root: "/guest".into(),
            backend: Backend::Nixos(NixosTarget {
                generation,
                specialisation: specialisation.map(str::to_owned),
                toplevel: "/nix/store/os".into(),
                init: "/nix/store/os/init".into(),
                kernel: "/kernel".into(),
                initrd: None,
                kernel_params: vec![],
            }),
            snapshot: None,
            mountpoint: None,
        };
        let targets = vec![
            target(8, None, "tank/root"),
            target(7, Some("niri"), "tank/root"),
            target(8, Some("niri"), "other/root"),
            target(8, Some("niri"), "tank/root"),
        ];
        assert_eq!(
            preferred_specialisation(&targets, Some(0), Some("niri")),
            Some(3)
        );
        assert_eq!(
            preferred_specialisation(&targets[..3], Some(0), Some("niri")),
            Some(0)
        );
        assert_eq!(preferred_specialisation(&targets, None, Some("niri")), None);
        assert_eq!(preferred_specialisation(&targets, Some(0), None), Some(0));
        assert_eq!(
            preferred_specialisation(&targets, Some(0), Some("absent")),
            Some(0)
        );
    }

    #[test]
    fn variants_preserve_identity_defaults_and_sibling_failures() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("zbm-variants-{}", std::process::id()));
        fs::create_dir_all(root.join("nix/var/nix/profiles")).unwrap();
        let parent = root.join("nix/store/base");
        fs::create_dir_all(parent.join("specialisation")).unwrap();
        let document = |name: &str| {
            serde_json::json!({"org.nixos.bootspec.v1": {
                "system": "x86_64-linux", "kernel": "/nix/store/shared/kernel",
                "init": format!("/nix/store/{name}/init"), "toplevel": format!("/nix/store/{name}"),
                "kernelParams": [], "label": name
            }})
        };
        let child = document("child");
        fs::create_dir_all(root.join("nix/store/child")).unwrap();
        fs::write(
            root.join("nix/store/child/boot.json"),
            serde_json::to_vec(&child).unwrap(),
        )
        .unwrap();
        symlink("/nix/store/child", parent.join("specialisation/niri")).unwrap();
        symlink("/nix/store/child", parent.join("specialisation/wrong")).unwrap();
        let mut base = document("base");
        base["org.nixos.specialisation.v1"] = serde_json::json!({
            "niri": child, "wrong": document("different"), "../escape": child
        });
        fs::write(parent.join("boot.json"), serde_json::to_vec(&base).unwrap()).unwrap();
        symlink(
            "/nix/store/base",
            root.join("nix/var/nix/profiles/system-7-link"),
        )
        .unwrap();
        symlink("system-7-link", root.join("nix/var/nix/profiles/system")).unwrap();
        let discovery = discover_generations(&root, "tank/root", 1).unwrap();
        assert_eq!(discovery.targets.len(), 2);
        assert_eq!(discovery.default_target, Some(0));
        let parent = &discovery.targets[0];
        let child = &discovery.targets[1];
        assert_eq!(parent.specialisation(), None);
        assert_eq!(child.specialisation(), Some("niri"));
        assert_eq!(parent.inputs().kernel, child.inputs().kernel);
        assert!(!parent.same_identity(child));
        assert_eq!(discovery.rejected.len(), 2);
        assert_eq!(
            discovery.rejected[0].specialisation.as_deref(),
            Some("../escape")
        );
        assert!(discovery.rejected[1].error.contains("parent declaration"));
        let roundtrip: BootTarget =
            serde_json::from_value(serde_json::to_value(child).unwrap()).unwrap();
        assert!(child.same_identity(&roundtrip));
        fs::remove_dir_all(root).unwrap();
    }

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
        symlink("system-3-link", root.join("nix/var/nix/profiles/system")).unwrap();
        let rollback = discover_generations(&root, "tank/root", 1).unwrap();
        assert_eq!(rollback.targets[0].generation(), Some(3));
        assert_eq!(rollback.default_target, Some(0));
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
        let prepared = root.join("boot/zbm-rs/nixos/os");
        fs::create_dir_all(&prepared).unwrap();
        fs::write(prepared.join("kernel"), b"signed-kernel").unwrap();
        fs::copy(root.join("nix/store/os/initrd"), prepared.join("initrd")).unwrap();
        let record = serde_json::json!({"version":1,
            "bootspec_sha256":crate::security::hex(&Sha256::digest(fs::read(&path).unwrap()))});
        fs::write(
            prepared.join("prepared.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let native = BootPlan::resolve_verified(&targets[0], &[]).unwrap();
        assert_eq!(
            BootPlan::resolve(&targets[0], &[]).unwrap().cmdline,
            native.cmdline
        );
        assert_eq!(native.kernel, prepared.join("kernel"));
        assert_eq!(native.initrd, Some(prepared.join("initrd")));
        // Bootspec changes must invalidate a stale prepared generation.
        document["org.nixos.bootspec.v1"]["kernelParams"] = serde_json::json!(["quiet"]);
        fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(BootPlan::resolve(&targets[0], &[]).is_err());
        fs::remove_dir_all(prepared).unwrap();
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
