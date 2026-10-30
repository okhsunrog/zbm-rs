use serde::{Deserialize, Serialize};

/// Only these invariants affect the executable; ordinary settings belong to the image.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BuildPolicy {
    pub security_mode: SecurityMode,
    pub diagnostic_esp_partuuid: Option<String>,
    pub continuous_diagnostics: bool,
}

impl BuildPolicy {
    pub fn validate(&self) -> Result<(), String> {
        if self.continuous_diagnostics && self.diagnostic_esp_partuuid.is_none() {
            return Err("diagnostics.continuous requires diagnostics.esp_partuuid".into());
        }
        if self.diagnostic_esp_partuuid.as_ref().is_some_and(|id| {
            id.len() != 36
                || id.bytes().enumerate().any(|(i, ch)| {
                    if [8, 13, 18, 23].contains(&i) {
                        ch != b'-'
                    } else {
                        !ch.is_ascii_hexdigit()
                    }
                })
        }) {
            return Err("diagnostics.esp_partuuid must be a GPT partition UUID".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SecurityMode {
    #[default]
    Off,
    Enforce,
}
