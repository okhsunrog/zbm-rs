//! Stable telemetry/broker representation; never deserialize unchecked domain states.
use super::*;

#[derive(Serialize, Deserialize)]
pub(super) struct TargetRecord {
    dataset: String,
    generation: Option<u64>,
    label: String,
    root: PathBuf,
    toplevel: Option<PathBuf>,
    backend: BackendRecord,
    snapshot: Option<SnapshotSource>,
    mountpoint: Option<String>,
    issue: Option<String>,
    inputs: BootInputs,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind")]
enum BackendRecord {
    Nixos,
    Linux {
        rootprefix: String,
        commandline: Vec<String>,
    },
}

impl From<BootTarget> for TargetRecord {
    fn from(target: BootTarget) -> Self {
        Self {
            generation: target.generation(),
            toplevel: target.toplevel().map(Path::to_owned),
            inputs: target.inputs(),
            issue: target.issue().map(str::to_owned),
            dataset: target.dataset,
            label: target.label,
            root: target.root,
            snapshot: target.snapshot,
            mountpoint: target.mountpoint,
            backend: match target.backend {
                Backend::Nixos(_) => BackendRecord::Nixos,
                Backend::Linux(t) => BackendRecord::Linux {
                    rootprefix: t.rootprefix,
                    commandline: t.commandline,
                },
            },
        }
    }
}

impl TryFrom<TargetRecord> for BootTarget {
    type Error = &'static str;
    fn try_from(record: TargetRecord) -> Result<Self, Self::Error> {
        let inputs = record.inputs;
        let backend = match record.backend {
            BackendRecord::Nixos => {
                if record.issue.is_some() {
                    return Err("NixOS target cannot carry a Linux discovery issue");
                }
                Backend::Nixos(NixosTarget {
                    generation: record.generation.ok_or("Missing NixOS generation")?,
                    toplevel: record.toplevel.ok_or("Missing NixOS toplevel")?,
                    init: inputs.init.ok_or("Missing NixOS init")?,
                    kernel: inputs.kernel,
                    initrd: inputs.initrd,
                    kernel_params: inputs.kernel_params,
                })
            }
            BackendRecord::Linux {
                rootprefix,
                commandline,
            } => {
                if record.generation.is_some() || record.toplevel.is_some() || inputs.init.is_some()
                {
                    return Err("Linux target cannot carry NixOS selectors");
                }
                if inputs.kernel_params != commandline {
                    return Err("Inconsistent Linux command line");
                }
                Backend::Linux(LinuxTarget {
                    kernel: inputs.kernel,
                    initrd: inputs.initrd,
                    rootprefix,
                    commandline,
                    issue: record.issue,
                })
            }
        };
        Ok(Self {
            dataset: record.dataset,
            label: record.label,
            root: record.root,
            snapshot: record.snapshot,
            mountpoint: record.mountpoint,
            backend,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> serde_json::Value {
        serde_json::json!({
            "dataset": "tank/root", "generation": 1, "label": "test", "root": "/guest",
            "toplevel": "/system", "backend": {"kind": "Nixos"}, "snapshot": null,
            "mountpoint": "/", "issue": null,
            "inputs": {"kernel": "/kernel", "initrd": "/initrd", "init": "/init", "kernel_params": ["quiet"]}
        })
    }

    #[test]
    fn wire_format_round_trips_both_backends() {
        let nixos = record();
        let mut linux = nixos.clone();
        linux["backend"] =
            serde_json::json!({"kind":"Linux", "rootprefix":"zfs=", "commandline":["quiet"]});
        linux["generation"] = serde_json::Value::Null;
        linux["toplevel"] = serde_json::Value::Null;
        linux["inputs"]["init"] = serde_json::Value::Null;
        for value in [nixos, linux] {
            let target: BootTarget = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(target).unwrap(), value);
        }
    }

    #[test]
    fn incomplete_or_mixed_backend_states_are_rejected() {
        for field in ["generation", "toplevel"] {
            let mut value = record();
            value[field] = serde_json::Value::Null;
            assert!(serde_json::from_value::<BootTarget>(value).is_err());
        }
        let mut value = record();
        value["inputs"]["init"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<BootTarget>(value).is_err());
        let mut value = record();
        value["backend"] =
            serde_json::json!({"kind":"Linux", "rootprefix":"zfs=", "commandline":["quiet"]});
        assert!(serde_json::from_value::<BootTarget>(value.clone()).is_err());
        value["generation"] = serde_json::Value::Null;
        value["toplevel"] = serde_json::Value::Null;
        value["inputs"]["init"] = serde_json::Value::Null;
        value["inputs"]["kernel_params"] = serde_json::json!(["different"]);
        assert!(serde_json::from_value::<BootTarget>(value).is_err());
    }
}
