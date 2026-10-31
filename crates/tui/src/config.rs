//! Image-owned JSON, read once per process. No config writer or reload service.
mod build_policy;
mod schema;
pub use schema::*;

pub const IMAGE_PATH: &str = "/etc/zbm-rs/config.json";
pub fn image_enforced() -> bool {
    env!("ZBM_IMAGE_SECURITY_MODE") == "enforce"
}

pub fn parse(input: &str) -> anyhow::Result<Config> {
    let config: Config = serde_json::from_str(input)?;
    config.validate().map_err(anyhow::Error::msg)?;
    Ok(config)
}

pub fn load(supervised: bool) -> anyhow::Result<Config> {
    use anyhow::Context;
    let path = if supervised {
        Some(std::path::PathBuf::from(IMAGE_PATH))
    } else {
        std::env::var_os("ZBM_RS_CONFIG").map(std::path::PathBuf::from)
    };
    let config = match path {
        Some(path) => parse(
            &std::fs::read_to_string(&path)
                .with_context(|| format!("reading config {}", path.display()))?,
        )
        .with_context(|| format!("invalid config {}", path.display())),
        None => Ok(Config::default()),
    }?;
    if supervised && (config.security.mode == SecurityMode::Enforce) != image_enforced() {
        anyhow::bail!("image security mode differs from the compiled image policy");
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_defaults_and_escaped_strings_round_trip() {
        let config = parse(r#"{"ui":{"title":"Boot \"Linux\""},"zfs":{"import_policy":"read-only"},"kernel_args":["quote=\"value\"","path=\\a","unicode=λ"]}"#).unwrap();
        assert_eq!(config.boot.timeout_secs, 5);
        assert!(matches!(config.zfs.import_policy, ImportPolicy::ReadOnly));
        let roundtrip = parse(&serde_json::to_string(&config).unwrap()).unwrap();
        assert_eq!(config.kernel_args, roundtrip.kernel_args);
        assert_eq!(config.ui.title, roundtrip.ui.title);
    }
    #[test]
    fn invalid_enum_unknown_fields_and_semantics_fail() {
        for (input, expected) in [
            (r#"{"zfs":{"import_policy":"unsafe"}}"#, "unknown variant"),
            (r#"{"boot":{"timeout_secs":301}}"#, "boot.timeout_secs"),
            (r#"{"boot":{"timeout_secs":0}}"#, "boot.timeout_secs"),
            (r#"{"boot":{"mode":"unsafe"}}"#, "unknown variant"),
            (
                r#"{"boot":{"preferred_pool_guid":"0"}}"#,
                "preferred_pool_guid",
            ),
            (
                r#"{"manager":{"restart_limit":9}}"#,
                "manager.restart_limit",
            ),
            (
                r#"{"nixos":{"generation_limit":0}}"#,
                "nixos.generation_limit",
            ),
            (r#"{"typo":1}"#, "unknown field"),
            (r#"{"ui":{"title":"\n"}}"#, "ui.title"),
            (
                r#"{"diagnostics":{"esp_partuuid":"../../sda"}}"#,
                "diagnostics.esp_partuuid",
            ),
            (r#"{"kernel_args":["\u0000"]}"#, "kernel_args"),
        ] {
            assert!(
                parse(input).unwrap_err().to_string().contains(expected),
                "{input}"
            );
        }
    }

    #[test]
    fn persistent_diagnostics_are_opt_in_and_use_only_explicit_gpt_ids() {
        assert!(parse("{}").unwrap().diagnostics.esp_partuuid.is_none());
        assert!(!parse("{}").unwrap().diagnostics.continuous);
        assert!(parse(r#"{"diagnostics":{"continuous":true}}"#).is_err());
        assert!(parse(r#"{"diagnostics":{"continuous":true,"esp_partuuid":"ed5d4b72-0aac-48d3-8f89-43362d952ca0"}}"#).is_ok());
        assert!(
            parse(r#"{"diagnostics":{"esp_partuuid":"ed5d4b72-0aac-48d3-8f89-43362d952ca0"}}"#)
                .is_ok()
        );
    }

    #[test]
    fn resume_is_opt_in_and_rejects_disk_logging() {
        assert!(parse("{}").unwrap().resume.swap_partuuid.is_none());
        let mut value = serde_json::json!({"resume": {"swap_partuuid": "12345678-1234-1234-1234-123456789abc"}});
        assert!(parse(&value.to_string()).is_ok());
        value["diagnostics"] =
            serde_json::json!({"esp_partuuid": "12345678-1234-1234-1234-123456789def"});
        assert!(parse(&value.to_string()).is_err());
        value.as_object_mut().unwrap().remove("diagnostics");
        value["security"] = serde_json::json!({"mode": "enforce", "target_authorities": ["/etc/zbm-rs/trust/owner.pem"], "ima_certificate": "/etc/zbm-rs/trust/ima.der"});
        // The canonical image builder validates the protected kernel, while
        // the broker requires the signed native-resume profile at handoff.
        assert!(parse(&value.to_string()).is_ok());
        value.as_object_mut().unwrap().remove("security");
        value["resume"]["swap_partuuid"] = "../../sda".into();
        assert!(parse(&value.to_string()).is_err());
    }

    #[test]
    fn encrypted_resume_requires_a_supported_explicit_credential() {
        let mut value = serde_json::json!({"resume": {
            "device": {"type":"luks2", "partuuid":"12345678-1234-1234-1234-123456789abc", "luks_uuid":"ca55a30a-636e-4e69-9e68-bb36bf380408", "mapping":"resume-swap"},
            "credential":{"type":"zfs-unlock", "pool_guid":"18446744073709551615", "encryption_root":"tank/root"}
        }});
        assert!(parse(&value.to_string()).is_ok());
        value["resume"]["credential"]["type"] = "zfs-keyfile".into();
        assert!(
            parse(&value.to_string())
                .unwrap_err()
                .to_string()
                .contains("unknown variant")
        );
        value["resume"]["credential"]["type"] = "zfs-unlock".into();
        value["resume"]["credential"]["pool_guid"] = "0".into();
        assert!(parse(&value.to_string()).is_err());
        value["resume"]["credential"]["pool_guid"] = "42".into();
        value["resume"]["swap_partuuid"] = "12345678-1234-1234-1234-123456789abc".into();
        assert!(parse(&value.to_string()).is_err());
        value["resume"]
            .as_object_mut()
            .unwrap()
            .remove("swap_partuuid");
        value["resume"]
            .as_object_mut()
            .unwrap()
            .remove("credential");
        assert!(parse(&value.to_string()).is_err());
    }

    #[test]
    fn enforced_configuration_requires_trust_and_valid_tpm_policy() {
        assert!(parse(r#"{"security":{"mode":"enforce"}}"#).is_err());
        assert!(parse(r#"{"security":{"require_firmware_secure_boot":true}}"#).is_err());
        let valid = r#"{"security":{"mode":"enforce","target_authorities":["/etc/zbm-rs/trust/owner.pem"],"ima_certificate":"/etc/zbm-rs/trust/ima.der","tpm":{"policy":"optional"}}}"#;
        assert!(parse(valid).is_ok());
        assert!(parse(&valid.replace("owner.pem", "../owner.pem")).is_err());
        for policy in ["off", "optional", "required"] {
            assert!(parse(&valid.replace("optional", policy)).is_ok());
        }
        assert!(parse(&valid.replace("optional", "unsupported")).is_err());
        // Removed loader-owned NvPCR settings must fail explicitly, never be ignored.
        for field in ["nvpcrs", "required_nvpcrs"] {
            let mut value: serde_json::Value = serde_json::from_str(valid).unwrap();
            value["security"]["tpm"][field] = serde_json::json!([]);
            assert!(parse(&value.to_string()).is_err());
        }
    }
}
