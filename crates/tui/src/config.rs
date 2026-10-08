//! Image-owned JSON, read once per process. No config writer or reload service.
mod schema;
pub use schema::*;

pub const IMAGE_PATH: &str = "/etc/zbm-rs/config.json";
pub fn image_enforced() -> bool {
    env!("ZBM_IMAGE_SECURITY_MODE") == "enforce"
}
const DEFAULT_JSON: &str = include_str!("../../../config/default.json");

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
        None => parse(DEFAULT_JSON),
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
        assert_eq!(config.ui.timeout_secs, 5);
        assert!(matches!(config.zfs.import_policy, ImportPolicy::ReadOnly));
        let roundtrip = parse(&serde_json::to_string(&config).unwrap()).unwrap();
        assert_eq!(config.kernel_args, roundtrip.kernel_args);
        assert_eq!(config.ui.title, roundtrip.ui.title);
    }
    #[test]
    fn invalid_enum_unknown_fields_and_semantics_fail() {
        for (input, expected) in [
            (r#"{"zfs":{"import_policy":"unsafe"}}"#, "unknown variant"),
            (r#"{"ui":{"timeout_secs":301}}"#, "ui.timeout_secs"),
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
            (r#"{"kernel_args":["\u0000"]}"#, "kernel_args"),
        ] {
            assert!(
                parse(input).unwrap_err().to_string().contains(expected),
                "{input}"
            );
        }
    }

    #[test]
    fn enforced_configuration_requires_trust_and_valid_tpm_selection() {
        assert!(parse(r#"{"security":{"mode":"enforce"}}"#).is_err());
        assert!(parse(r#"{"security":{"require_firmware_secure_boot":true}}"#).is_err());
        let valid = r#"{"security":{"mode":"enforce","target_authorities":["/etc/zbm-rs/trust/owner.pem"],"ima_certificate":"/etc/zbm-rs/trust/ima.der","tpm":{"policy":"optional","nvpcrs":["hardware","login"],"required_nvpcrs":["hardware"]}}}"#;
        assert!(parse(valid).is_ok());
        assert!(parse(&valid.replace("owner.pem", "../owner.pem")).is_err());
        assert!(
            parse(&valid.replace("\"hardware\",\"login\"", "\"hardware\",\"hardware\"")).is_err()
        );
        assert!(parse(&valid.replace("\"optional\"", "\"off\"")).is_err());
    }
}
