//! Pure boot policy. These states carry guest paths, never executable authority.
//! Only the I/O boundary can resolve a GuestPlan into a BootPlan; authorization
//! and kexec remain separate, effectful stages.
use super::*;

pub(super) struct NixosConfiguration {
    spec: Bootspec,
    fstab: String,
}

pub(super) struct CheckedNixos {
    target: BootTarget,
    inputs: BootInputs,
    cmdline: Vec<String>,
    root: RootSelection,
    additional_initrd_mounts: bool,
}

pub(super) enum InitrdSupport {
    Systemd,
    Unsupported,
}

enum RootSelection {
    Fstab,
    Explicit(String),
}

/// A policy-checked plan whose paths still belong to the selected OS.
#[derive(Debug)]
pub(crate) struct GuestPlan {
    pub(super) target: BootTarget,
    pub(super) inputs: BootInputs,
    pub(super) cmdline: Vec<String>,
}

impl NixosConfiguration {
    pub(super) fn new(spec: Bootspec, fstab: String) -> Self {
        Self { spec, fstab }
    }

    pub(super) fn check(
        self,
        target: BootTarget,
        extra: &[String],
        root_override: Option<&str>,
    ) -> io::Result<CheckedNixos> {
        let mounts: Vec<Vec<&str>> = self
            .fstab
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .map(|line| line.split_whitespace().collect())
            .filter(|fields: &Vec<_>| fields.len() >= 3)
            .collect();
        let roots: Vec<_> = mounts.iter().filter(|fields| fields[1] == "/").collect();
        let [root] = roots.as_slice() else {
            return Err(invalid(
                "NixOS configuration root does not match selected ZFS dataset",
            ));
        };
        if root[2] != "zfs" {
            return Err(invalid(
                "NixOS configuration root does not match selected ZFS dataset",
            ));
        }
        if mounts
            .iter()
            .any(|fields| fields[1] == "/nix" || fields[1].starts_with("/nix/"))
        {
            return Err(invalid("Separate Nix store mounts are not supported yet"));
        }
        let root = match root_override {
            Some(dataset) => RootSelection::Explicit(dataset.into()),
            None if root[0] != target.dataset => RootSelection::Explicit(target.dataset.clone()),
            None => RootSelection::Fstab,
        };
        let inputs = BootInputs::from_spec(&self.spec);
        let cmdline = inputs.command_line(extra);
        if cmdline.iter().any(|argument| {
            argument.is_empty()
                || argument
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\'')
        }) {
            return Err(invalid("Unsupported whitespace/quoting in kernel argument"));
        }
        if extra
            .iter()
            .any(|arg| arg.starts_with("init=") || arg.starts_with("root="))
        {
            return Err(invalid("Cannot override Bootspec init or root"));
        }
        if cmdline
            .iter()
            .any(|arg| arg.starts_with("root=") && arg != "root=fstab")
        {
            return Err(invalid("Only the configured NixOS fstab root is supported"));
        }
        let additional_initrd_mounts = mounts.iter().any(|fields| {
            fields[1] != "/"
                && (fields[1] == "/usr"
                    || fields.get(3).is_some_and(|options| {
                        options.split(',').any(|option| option == "x-initrd.mount")
                    }))
        });
        Ok(CheckedNixos {
            target,
            inputs,
            cmdline,
            root,
            additional_initrd_mounts,
        })
    }
}

impl CheckedNixos {
    /// Request inspection only when root rewriting needs a systemd initrd.
    pub(super) fn required_initrd(&self) -> io::Result<Option<&Path>> {
        match self.root {
            RootSelection::Fstab => Ok(None),
            RootSelection::Explicit(_) => self
                .inputs
                .initrd
                .as_deref()
                .map(Some)
                .ok_or_else(|| invalid("NixOS clone boot requires an initrd")),
        }
    }

    pub(super) fn plan(self, initrd: InitrdSupport) -> io::Result<GuestPlan> {
        let Self {
            target,
            inputs,
            cmdline,
            root,
            additional_initrd_mounts,
        } = self;
        let RootSelection::Explicit(dataset) = root else {
            return Ok(GuestPlan {
                target,
                inputs,
                cmdline,
            });
        };
        if !matches!(initrd, InitrdSupport::Systemd) {
            return Err(invalid("NixOS root override requires systemd initrd"));
        }
        if inputs.initrd.is_none() {
            return Err(invalid("NixOS clone boot requires an initrd"));
        }
        zfskit::names::DatasetName::parse(&dataset).map_err(invalid_error)?;
        if additional_initrd_mounts {
            return Err(invalid(
                "Snapshot boot does not support additional initrd mounts",
            ));
        }
        if cmdline.iter().any(|arg| {
            arg.starts_with("rootflags=")
                || arg.starts_with("rootfstype=")
                || arg.starts_with("rd.fstab=")
                || arg == "ro"
        }) {
            return Err(invalid(
                "Snapshot boot cannot override conflicting root policy",
            ));
        }
        let rootflags = if dataset == target.dataset
            && target.mountpoint.as_deref().is_some_and(|p| p != "legacy")
        {
            "rootflags=defaults,zfsutil"
        } else {
            "rootflags=defaults"
        };
        let cmdline = cmdline
            .into_iter()
            .filter(|arg| !arg.starts_with("root="))
            .chain([
                format!("root={dataset}"),
                "rootfstype=zfs".into(),
                rootflags.into(),
                "rw".into(),
                "rd.fstab=no".into(),
            ])
            .collect();
        Ok(GuestPlan {
            target: BootTarget { dataset, ..target },
            inputs,
            cmdline,
        })
    }
}

impl GuestPlan {
    pub(crate) fn linux(target: &BootTarget, dataset: &str, extra: &[String]) -> io::Result<Self> {
        if let Some(issue) = target.issue() {
            return Err(invalid(issue));
        }
        zfskit::names::DatasetName::parse(dataset).map_err(invalid_error)?;
        let Backend::Linux(LinuxTarget {
            rootprefix,
            commandline,
            ..
        }) = &target.backend
        else {
            return Err(invalid("Not a Linux kernel target"));
        };
        let cmdline: Vec<_> = commandline
            .iter()
            .chain(extra)
            .filter(|arg| !arg.starts_with("root=") && !arg.starts_with("zfs="))
            .cloned()
            .chain([format!("{rootprefix}{dataset}")])
            .collect();
        crate::linux::command_line(&cmdline)?;
        Ok(Self {
            target: target.clone(),
            inputs: target.inputs(),
            cmdline,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configuration(fstab: &str) -> NixosConfiguration {
        NixosConfiguration::new(
            Bootspec {
                system: "x86_64-linux".into(),
                kernel: "/kernel".into(),
                initrd: Some("/initrd".into()),
                initrd_secrets: None,
                init: "/init".into(),
                toplevel: "/system".into(),
                kernel_params: vec!["root=fstab".into()],
                label: "test".into(),
            },
            fstab.into(),
        )
    }

    fn target() -> BootTarget {
        BootTarget {
            dataset: "tank/root".into(),
            label: "test".into(),
            root: "/nonexistent-pure-policy-fixture".into(),
            snapshot: None,
            mountpoint: Some("/".into()),
            backend: Backend::Nixos(NixosTarget {
                generation: 1,
                toplevel: "/system".into(),
                kernel: "/stale-kernel".into(),
                initrd: None,
                init: "/init".into(),
                kernel_params: vec![],
            }),
        }
    }

    #[test]
    fn fstab_plan_uses_fresh_spec_and_needs_no_initrd_inspection() {
        let checked = configuration("tank/root / zfs defaults 0 0")
            .check(target(), &["quiet".into()], None)
            .unwrap();
        assert!(checked.required_initrd().unwrap().is_none());
        let plan = checked.plan(InitrdSupport::Unsupported).unwrap();
        assert_eq!(plan.inputs.kernel, Path::new("/kernel"));
        assert_eq!(plan.cmdline, ["init=/init", "root=fstab", "quiet"]);
        assert_eq!(plan.target.dataset, "tank/root");
    }

    #[test]
    fn explicit_root_requires_systemd_and_preserves_clone_identity() {
        let checked = || {
            configuration("tank/root / zfs defaults 0 0")
                .check(target(), &[], Some("tank/clone"))
                .unwrap()
        };
        assert_eq!(
            checked().required_initrd().unwrap(),
            Some(Path::new("/initrd"))
        );
        assert!(checked().plan(InitrdSupport::Unsupported).is_err());
        let plan = checked().plan(InitrdSupport::Systemd).unwrap();
        assert_eq!(plan.target.dataset, "tank/clone");
        assert_eq!(
            plan.cmdline,
            [
                "init=/init",
                "root=tank/clone",
                "rootfstype=zfs",
                "rootflags=defaults",
                "rw",
                "rd.fstab=no"
            ]
        );
    }

    #[test]
    fn relocated_and_verified_roots_use_native_mountpoint_policy() {
        for (fstab, override_root) in [
            ("tank/old / zfs defaults", None),
            ("tank/root / zfs defaults", Some("tank/root")),
        ] {
            for (mountpoint, flags) in [
                ("/", "rootflags=defaults,zfsutil"),
                ("legacy", "rootflags=defaults"),
            ] {
                let target = BootTarget {
                    mountpoint: Some(mountpoint.into()),
                    ..target()
                };
                let plan = configuration(fstab)
                    .check(target, &[], override_root)
                    .unwrap()
                    .plan(InitrdSupport::Systemd)
                    .unwrap();
                assert!(plan.cmdline.contains(&"root=tank/root".into()));
                assert!(plan.cmdline.contains(&flags.into()));
            }
        }
    }

    #[test]
    fn root_rewrite_rejects_conflicts_and_additional_early_mounts() {
        for arg in ["rootflags=ro", "rootfstype=ext4", "rd.fstab=yes", "ro"] {
            let checked = configuration("tank/root / zfs defaults")
                .check(target(), &[arg.into()], Some("tank/clone"))
                .unwrap();
            assert!(checked.plan(InitrdSupport::Systemd).is_err(), "{arg}");
        }
        for mount in [
            "tank/usr /usr zfs defaults",
            "tank/data /data zfs x-initrd.mount",
        ] {
            let fstab = format!("tank/root / zfs defaults\n{mount}");
            assert!(
                configuration(&fstab)
                    .check(target(), &[], Some("tank/clone"))
                    .unwrap()
                    .plan(InitrdSupport::Systemd)
                    .is_err()
            );
            assert!(
                configuration(&fstab)
                    .check(target(), &[], None)
                    .unwrap()
                    .plan(InitrdSupport::Unsupported)
                    .is_ok()
            );
        }
    }

    #[test]
    fn invalid_fstab_and_unsafe_arguments_never_reach_planning() {
        for fstab in [
            "",
            "tank/root / ext4 defaults",
            "tank/root / zfs defaults\ntank/other / zfs defaults",
            "tank/root / zfs defaults\ntank/nix /nix zfs defaults",
            "tank/root / zfs defaults\ntank/store /nix/store zfs defaults",
        ] {
            assert!(
                configuration(fstab).check(target(), &[], None).is_err(),
                "{fstab}"
            );
        }
        for arg in [
            "init=/other",
            "root=other",
            "two words",
            "",
            "quote'",
            "line\nbreak",
        ] {
            assert!(
                configuration("tank/root / zfs defaults")
                    .check(target(), &[arg.into()], None)
                    .is_err(),
                "{arg}"
            );
        }
        let mut no_initrd = configuration("tank/root / zfs defaults");
        no_initrd.spec.initrd = None;
        assert!(
            no_initrd
                .check(target(), &[], Some("tank/clone"))
                .unwrap()
                .required_initrd()
                .is_err()
        );
    }

    #[test]
    fn linux_policy_replaces_all_root_selectors_without_filesystem_reads() {
        let target = BootTarget {
            backend: Backend::Linux(LinuxTarget {
                kernel: "/kernel".into(),
                initrd: None,
                issue: None,
                rootprefix: "zfs=".into(),
                commandline: vec!["root=old".into(), "quiet".into()],
            }),
            ..target()
        };
        let plan = GuestPlan::linux(
            &target,
            "tank/clone",
            &["zfs=other".into(), "label=two words".into()],
        )
        .unwrap();
        assert_eq!(plan.cmdline, ["quiet", "label=two words", "zfs=tank/clone"]);
        assert!(GuestPlan::linux(&target, "invalid@clone", &[]).is_err());
        let mut broken = target;
        if let Backend::Linux(linux) = &mut broken.backend {
            linux.issue = Some("broken".into());
        }
        assert!(GuestPlan::linux(&broken, "tank/root", &[]).is_err());
    }
}
