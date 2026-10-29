#[path = "src/config/schema.rs"]
#[allow(dead_code)]
mod schema;
fn main() {
    println!("cargo::rerun-if-env-changed=ZBM_RS_BUILD_POLICY");
    println!("cargo::rerun-if-changed=src/config/schema.rs");
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let input = match std::env::var("ZBM_RS_BUILD_POLICY") {
            Ok(input) => input,
            Err(std::env::VarError::NotPresent) => "{}".into(),
            Err(error) => return Err(error.into()),
        };
        let policy: schema::BuildPolicy = serde_json::from_str(&input)?;
        policy.validate()?;
        println!(
            "cargo::rustc-env=ZBM_CONTINUOUS_DIAGNOSTICS={}",
            policy.continuous_diagnostics
        );
        println!(
            "cargo::rustc-env=ZBM_DIAGNOSTIC_ESP_PARTUUID={}",
            policy.diagnostic_esp_partuuid.as_deref().unwrap_or("")
        );
        println!(
            "cargo::rustc-env=ZBM_IMAGE_SECURITY_MODE={}",
            match policy.security_mode {
                schema::SecurityMode::Off => "off",
                schema::SecurityMode::Enforce => "enforce",
            }
        );
        Ok(())
    })();
    if let Err(error) = result {
        panic!("invalid zbm-rs build policy: {error}");
    }
}
