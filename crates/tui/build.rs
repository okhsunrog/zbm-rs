#[path = "src/config/schema.rs"]
mod schema;
fn main() {
    println!("cargo::rerun-if-env-changed=ZBM_RS_CONFIG");
    println!("cargo::rerun-if-changed=src/config/schema.rs");
    let path = std::env::var_os("ZBM_RS_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/default.json")
        });
    println!("cargo::rerun-if-changed={}", path.display());
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let input = std::fs::read_to_string(&path)?;
        let config: schema::Config = serde_json::from_str(&input)?;
        config.validate()?;
        Ok(())
    })();
    if let Err(error) = result {
        panic!(
            "invalid zbm-rs build configuration {}: {error}",
            path.display()
        );
    }
}
