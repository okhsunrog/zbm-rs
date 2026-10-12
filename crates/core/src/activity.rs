//! Operation lifecycle shared by the runtime and its read-only UI projection.
use serde::{Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    Retry,
    RestartRequired,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Activity<T = String> {
    #[default]
    Idle,
    Scanning,
    Running(T),
    RestartRequired,
}

impl<T> Activity<T> {
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }
    pub fn is_scanning(&self) -> bool {
        matches!(self, Self::Scanning)
    }
    pub fn requires_restart(&self) -> bool {
        matches!(self, Self::RestartRequired)
    }
    pub fn operation(&self) -> Option<&T> {
        match self {
            Self::Running(operation) => Some(operation),
            _ => None,
        }
    }
    pub fn project<U>(&self, f: impl FnOnce(&T) -> U) -> Activity<U> {
        match self {
            Self::Idle => Activity::Idle,
            Self::Scanning => Activity::Scanning,
            Self::Running(operation) => Activity::Running(f(operation)),
            Self::RestartRequired => Activity::RestartRequired,
        }
    }
    pub fn recovered(action: RecoveryAction) -> Self {
        match action {
            RecoveryAction::Retry => Self::Idle,
            RecoveryAction::RestartRequired => Self::RestartRequired,
        }
    }
}

// Preserve telemetry fields without making independently mutable flags.
impl Serialize for Activity<String> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Record<'a> {
            scanning: bool,
            operation: Option<&'a String>,
            requires_restart: bool,
        }
        Record {
            scanning: self.is_scanning(),
            operation: self.operation(),
            requires_restart: self.requires_restart(),
        }
        .serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn telemetry_flags_are_mutually_exclusive_projections() {
        for phase in [
            Activity::Idle,
            Activity::Scanning,
            Activity::Running("loading".into()),
            Activity::RestartRequired,
        ] {
            let value = serde_json::to_value(&phase).unwrap();
            let active = usize::from(value["scanning"] == true)
                + usize::from(!value["operation"].is_null())
                + usize::from(value["requires_restart"] == true);
            assert_eq!(active, usize::from(!phase.is_idle()));
        }
        assert!(Activity::<()>::recovered(RecoveryAction::RestartRequired).requires_restart());
    }
}
