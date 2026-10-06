use std::process::Command;
fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_zbm-rs"))
}
#[test]
fn default_and_alternate_image_config_are_typed_and_validated() {
    let output = binary()
        .env_remove("ZBM_RS_CONFIG")
        .arg("--print-config")
        .output()
        .unwrap();
    assert!(output.status.success());
    let default: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(default["manager"]["restart_limit"], 2);
    assert_eq!(default["ui"]["timeout_secs"], 5);
    assert!(default["ui"]["title"].is_null());
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/test.json");
    let output = binary()
        .env("ZBM_RS_CONFIG", &fixture)
        .arg("--print-config")
        .output()
        .unwrap();
    assert!(output.status.success());
    let alternate: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(alternate["zfs"]["import_policy"], "read-only");
    assert_eq!(alternate["ui"]["title"], "zbm-rs test");
    assert_eq!(
        alternate["kernel_args"],
        serde_json::json!(["quiet", "zbm.test=1"])
    );
}
#[test]
fn malformed_config_never_silently_falls_back_to_defaults() {
    let output = binary()
        .env("ZBM_RS_CONFIG", "/nonexistent-zbm-config.json")
        .arg("--print-config")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("reading config"));
}
