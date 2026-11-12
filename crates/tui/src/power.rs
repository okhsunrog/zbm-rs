//! Read-only early-boot power information. Missing telemetry is never zero charge.
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Power {
    pub batteries: Vec<Battery>,
    pub external: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Battery {
    pub capacity: Option<u8>,
    pub status: String,
}

impl Power {
    pub fn read() -> Self {
        Self::read_from(Path::new("/sys/class/power_supply"))
    }

    fn read_from(root: &Path) -> Self {
        let mut power = Self::default();
        let Ok(entries) = fs::read_dir(root) else {
            return power;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let read = |name| {
                fs::read_to_string(path.join(name))
                    .ok()
                    .map(|v| v.trim().to_owned())
            };
            // Peripheral batteries (mice, headsets, etc.) cannot power the laptop.
            if read("scope").as_deref() == Some("Device") || read("present").as_deref() == Some("0")
            {
                continue;
            }
            match read("type").as_deref() {
                Some("Battery") => power.batteries.push(Battery {
                    capacity: read("capacity")
                        .and_then(|v| v.parse::<u8>().ok())
                        .filter(|v| *v <= 100),
                    status: match read("status").as_deref() {
                        Some("Charging") => "Charging",
                        Some("Discharging") => "Discharging",
                        Some("Full") => "Full",
                        Some("Not charging") => "Not charging",
                        _ => "Unknown",
                    }
                    .into(),
                }),
                Some(_) => power.external |= read("online").as_deref() == Some("1"),
                None => {}
            }
        }
        // Stable display even when sysfs enumeration order changes.
        power
            .batteries
            .sort_by(|a, b| a.capacity.cmp(&b.capacity).then(a.status.cmp(&b.status)));
        power
    }

    pub fn low(&self) -> bool {
        self.batteries
            .iter()
            .any(|b| b.capacity.is_some_and(|c| c < 5))
            && (!self.external || self.batteries.iter().any(|b| b.status == "Discharging"))
    }

    pub fn label(&self) -> Option<String> {
        if self.batteries.is_empty() {
            return None;
        }
        let values = self
            .batteries
            .iter()
            .map(|b| {
                let capacity = b
                    .capacity
                    .map_or_else(|| "Unknown".into(), |v| format!("{v}%"));
                format!("{capacity} · {}", b.status)
            })
            .collect::<Vec<_>>()
            .join(" / ");
        Some(format!("Battery: {values}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn power(capacity: Option<u8>, status: &str, external: bool) -> Power {
        Power {
            batteries: vec![Battery {
                capacity,
                status: status.into(),
            }],
            external,
        }
    }

    #[test]
    fn threshold_and_external_power() {
        for capacity in 0..=100 {
            assert_eq!(
                power(Some(capacity), "Discharging", false).low(),
                capacity < 5
            );
        }
        assert!(!power(Some(3), "Charging", true).low());
        assert!(!power(Some(3), "Not charging", true).low());
        assert!(power(Some(3), "Discharging", true).low());
        assert!(power(Some(3), "Unknown", false).low());
        assert!(!power(None, "Discharging", false).low());
        assert!(!Power::default().low());
        assert_eq!(Power::default().label(), None);
        assert!(
            power(None, "Unknown", false)
                .label()
                .unwrap()
                .contains("Unknown")
        );
        let mut multiple = power(Some(80), "Discharging", false);
        multiple.batteries.push(Battery {
            capacity: Some(3),
            status: "Discharging".into(),
        });
        assert!(multiple.low());
    }

    #[test]
    fn sysfs_missing_malformed_peripheral_and_hotplug() {
        let root = std::env::temp_dir().join(format!("zbm-power-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let write = |supply: &str, name: &str, value: &str| {
            fs::create_dir_all(root.join(supply)).unwrap();
            fs::write(root.join(supply).join(name), value).unwrap();
        };
        assert_eq!(Power::read_from(&root), Power::default());
        write("BAT1", "type", "Battery\n");
        write("BAT1", "capacity", "3\n");
        write("BAT1", "status", "Discharging\n");
        assert!(Power::read_from(&root).low());
        write("AC", "type", "Mains\n");
        write("AC", "online", "1\n");
        assert!(Power::read_from(&root).low());
        write("BAT1", "status", "Charging\n");
        assert!(!Power::read_from(&root).low());
        write("mouse", "type", "Battery");
        write("mouse", "scope", "Device");
        write("mouse", "capacity", "0");
        assert_eq!(Power::read_from(&root).batteries.len(), 1);
        for malformed in ["101", "-1", "", "unknown"] {
            write("BAT1", "capacity", malformed);
            assert_eq!(Power::read_from(&root).batteries[0].capacity, None);
        }
        fs::remove_file(root.join("BAT1/capacity")).unwrap();
        assert_eq!(Power::read_from(&root).batteries[0].capacity, None);
        write("BAT1", "present", "0");
        assert!(Power::read_from(&root).batteries.is_empty());
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(Power::read_from(&root), Power::default());
    }
}
