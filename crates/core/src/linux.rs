//! ZFSBootMenu on-disk kernel/initramfs and command-line conventions.
//! Filesystem reads only; no importing, mounting or ZFS mutations.
use crate::{
    boot::{Backend, BootInputs, BootPlan, BootTarget, TargetDiscovery, rooted_path},
    environment::BootProperties,
};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

pub fn arguments(value: &str) -> io::Result<Vec<String>> {
    if value.chars().any(char::is_control) {
        return Err(invalid("Control character in kernel command line"));
    }
    let args =
        shlex::split(value).ok_or_else(|| invalid("Unbalanced kernel command-line quotes"))?;
    validate(&args)?;
    Ok(args)
}
fn validate(args: &[String]) -> io::Result<()> {
    if args
        .iter()
        .any(|arg| arg.is_empty() || arg.chars().any(|c| c.is_control() || c == '"'))
    {
        return Err(invalid("Unsupported kernel argument"));
    }
    Ok(())
}
/// Linux next_arg accepts double-quoted words; preserve spaces without a shell.
pub fn command_line(args: &[String]) -> io::Result<String> {
    validate(args)?;
    Ok(args
        .iter()
        .map(|arg| {
            if arg.chars().any(char::is_whitespace) {
                format!("\"{arg}\"")
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" "))
}
fn root_argument(arg: &str) -> bool {
    ["root=", "zfs="]
        .iter()
        .any(|prefix| arg.starts_with(prefix))
}
fn regular(root: &Path, path: &Path) -> bool {
    rooted_path(root, path)
        .and_then(fs::metadata)
        .is_ok_and(|m| m.is_file())
}
fn prefix(root: &Path, properties: &BootProperties) -> io::Result<String> {
    let prefix = if let Some(value) = &properties.rootprefix {
        value.clone()
    } else {
        let release = ["/etc/os-release", "/usr/lib/os-release"]
            .into_iter()
            .find_map(|p| {
                rooted_path(root, Path::new(p))
                    .ok()
                    .and_then(|p| fs::read_to_string(p).ok())
            })
            .unwrap_or_default();
        let value = |key: &str| {
            release
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .unwrap_or("")
                .trim_matches(['"', '\''])
        };
        value("ID=")
            .split_whitespace()
            .chain(value("ID_LIKE=").split_whitespace())
            .find_map(|id| match id {
                "arch" | "artix" | "cachyos" => Some("zfs="),
                "gentoo" | "alpine" => Some("root=ZFS="),
                "void" | "ubuntu" | "debian" | "devuan" | "chimera" => Some("root=zfs:"),
                _ => None,
            })
            .unwrap_or("root=zfs:")
            .into()
    };
    if !["zfs=", "root="].iter().any(|p| prefix.starts_with(p))
        || prefix
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"')
    {
        return Err(invalid("Invalid org.zfsbootmenu:rootprefix"));
    }
    Ok(prefix)
}
fn version_key(name: &str) -> Vec<(bool, String)> {
    // Numeric runs sort by digit count then lexically, without integer overflow.
    let mut runs: Vec<(bool, String)> = vec![];
    for c in name.chars() {
        let digit = c.is_ascii_digit();
        if runs.last().is_none_or(|run| run.0 != digit) {
            runs.push((digit, String::new()));
        }
        runs.last_mut().unwrap().1.push(c);
    }
    for (digit, value) in &mut runs {
        if *digit {
            let digits = value.trim_start_matches('0');
            *value = format!("{:08}{}", digits.len(), digits);
        }
    }
    runs
}

pub fn discover(
    root: &Path,
    dataset: &str,
    properties: &BootProperties,
) -> io::Result<TargetDiscovery> {
    let boot = match rooted_path(root, Path::new("/boot")) {
        Ok(p) => p,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(TargetDiscovery::default()),
        Err(e) => return Err(e),
    };
    let mut names = vec![];
    for entry in fs::read_dir(boot)?.take(1025) {
        names.push(entry?.file_name().to_string_lossy().into_owned());
    }
    if names.len() > 1024 {
        return Err(invalid("Too many /boot entries"));
    }
    let rootprefix = prefix(root, properties)?;
    let mut targets = vec![];
    for name in &names {
        let Some(base) = ["vmlinuz", "vmlinux", "linux", "linuz", "kernel"]
            .into_iter()
            .find(|base| name == *base || name.starts_with(&format!("{base}-")))
        else {
            continue;
        };
        let kernel = PathBuf::from("/boot").join(name);
        if !regular(root, &kernel) {
            continue;
        }
        let suffix = name.strip_prefix(base).unwrap().trim_start_matches('-');
        let version = if suffix.is_empty() {
            String::new()
        } else {
            format!("-{suffix}")
        };
        let mut candidates = vec![
            format!("initramfs-{name}.img"),
            format!("initramfs{version}.img"),
            format!("initrd.img-{name}"),
            format!("initrd.img{version}"),
            format!("initramfs-{name}"),
            format!("initramfs{version}"),
        ];
        for extension in [".img", ""] {
            for compression in [
                "", ".gz", ".bz2", ".xz", ".lzma", ".lz4", ".lzo", ".zstd", ".zst",
            ] {
                let ext = format!("{extension}{compression}");
                for prefix in ["initramfs", "initrd"] {
                    for label in [format!("-{name}"), version.clone()] {
                        candidates.extend([
                            format!("{prefix}{label}{ext}"),
                            format!("{prefix}{ext}{label}"),
                        ]);
                    }
                }
            }
        }
        let initrd = candidates
            .into_iter()
            .map(|name| PathBuf::from("/boot").join(name))
            .find(|path| regular(root, path));
        let Some(initrd) = initrd else {
            continue;
        };
        let kcl = PathBuf::from("/boot").join(format!("{name}.kcl"));
        let commandline = (|| -> io::Result<Vec<String>> {
            match rooted_path(root, &kcl) {
                Ok(path) => {
                    if fs::metadata(&path)?.len() > 65536 {
                        return Err(invalid("Kernel command-line file too large"));
                    }
                    arguments(
                        &fs::read_to_string(path)?
                            .lines()
                            .next()
                            .unwrap_or("")
                            .replace("%{parent}", properties.commandline.as_deref().unwrap_or("")),
                    )
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => arguments(
                    properties
                        .commandline
                        .as_deref()
                        .unwrap_or("quiet loglevel=4"),
                ),
                Err(e) => Err(e),
            }
        })();
        let (commandline, issue) = match commandline {
            Ok(args) => (args, None),
            Err(error) => (vec![], Some(error.to_string())),
        };
        targets.push(BootTarget {
            dataset: dataset.into(),
            generation: None,
            label: name.clone(),
            root: root.into(),
            toplevel: None,
            snapshot: None,
            mountpoint: Some(properties.mountpoint.clone()),
            issue,
            inputs: BootInputs {
                kernel,
                initrd: Some(initrd),
                init: None,
                kernel_params: commandline.clone(),
            },
            backend: Backend::Linux {
                rootprefix: rootprefix.clone(),
                commandline,
            },
        });
    }
    targets.sort_by_key(|target| std::cmp::Reverse(version_key(&target.label)));
    if let Some(preferred) = &properties.kernel
        && let Some(index) = targets
            .iter()
            .position(|target| target.label.contains(preferred))
    {
        let target = targets.remove(index);
        targets.insert(0, target);
    }
    Ok(TargetDiscovery {
        targets,
        rejected: vec![],
    })
}

pub fn resolve(target: &BootTarget, dataset: &str, extra: &[String]) -> io::Result<BootPlan> {
    if let Some(issue) = &target.issue {
        return Err(invalid(issue.clone()));
    }
    zfskit::names::DatasetName::parse(dataset).map_err(|e| invalid(e.to_string()))?;
    let Backend::Linux {
        rootprefix,
        commandline,
    } = &target.backend
    else {
        return Err(invalid("Not a Linux kernel target"));
    };
    let mut args = commandline
        .iter()
        .chain(extra)
        .filter(|arg| !root_argument(arg))
        .cloned()
        .collect::<Vec<_>>();
    args.push(format!("{rootprefix}{dataset}"));
    validate(&args)?;
    let kernel = rooted_path(&target.root, &target.inputs.kernel)?;
    let initrd = target
        .inputs
        .initrd
        .as_ref()
        .map(|path| rooted_path(&target.root, path))
        .transpose()?;
    for path in std::iter::once(&kernel).chain(initrd.iter()) {
        if !fs::metadata(path)?.is_file() {
            return Err(invalid("Kernel/initrd is not a regular file"));
        }
    }
    Ok(BootPlan {
        target: target.clone(),
        kernel,
        initrd,
        cmdline: args,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_is_authoritative_and_quoted_values_survive() {
        assert_eq!(
            arguments("root=old zfs=old foo=\"two words\"").unwrap(),
            ["root=old", "zfs=old", "foo=two words"]
        );
        assert_eq!(
            command_line(&["foo=two words".into()]).unwrap(),
            "\"foo=two words\""
        );
        assert!(arguments("foo='unterminated").is_err());
        assert!(arguments("foo\nbar").is_err());
        assert!(version_key("linux-6.10") > version_key("linux-6.9"));
    }
    #[test]
    fn matches_zbm_pairs_preferences_and_guest_symlinks() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("zbm-linux-{}", std::process::id()));
        fs::create_dir_all(root.join("boot")).unwrap();
        fs::create_dir_all(root.join("etc")).unwrap();
        fs::write(
            root.join("etc/os-release"),
            "ID=\"cachyos\"\nID_LIKE='arch'\n",
        )
        .unwrap();
        for name in [
            "vmlinuz-linux-lts",
            "initramfs-linux-lts.img",
            "vmlinuz-6.9",
            "initrd.img-6.9",
            "vmlinuz-6.10",
            "initramfs-6.10.img.gz",
            "linux",
            "initramfs.img",
            "vmlinuz-orphan",
            "vmlinuz-bad",
            "initramfs-bad.img",
        ] {
            fs::write(root.join("boot").join(name), "fixture").unwrap();
        }
        symlink("../../outside", root.join("boot/vmlinuz-escape")).unwrap();
        fs::write(root.join("boot/initramfs-escape.img"), "fixture").unwrap();
        fs::write(
            root.join("boot/vmlinuz-linux-lts.kcl"),
            "%{parent} foo=\"two words\" root=wrong zfs=wrong\n",
        )
        .unwrap();
        fs::write(root.join("boot/vmlinuz-bad.kcl"), "bad=\"unterminated").unwrap();
        let properties = BootProperties {
            mountpoint: "/".into(),
            canmount: "noauto".into(),
            encryption: "off".into(),
            active: "-".into(),
            commandline: Some("rw".into()),
            kernel: Some("linux-lts".into()),
            rootprefix: None,
        };
        let targets = discover(&root, "tank/root", &properties).unwrap().targets;
        assert_eq!(targets.len(), 5);
        let rejected = targets.iter().find(|t| t.label == "vmlinuz-bad").unwrap();
        assert!(rejected.issue.is_some());
        assert!(resolve(rejected, "tank/root", &[]).is_err());
        assert_eq!(targets[0].label, "vmlinuz-linux-lts");
        assert!(
            targets
                .iter()
                .all(|t| t.generation.is_none() && t.toplevel.is_none())
        );
        let plan = resolve(&targets[0], "tank/clone", &["root=extra-wrong".into()]).unwrap();
        assert_eq!(plan.cmdline, ["rw", "foo=two words", "zfs=tank/clone"]);
        assert!(plan.initrd.unwrap().ends_with("initramfs-linux-lts.img"));
        fs::remove_dir_all(root).unwrap();
    }
}
