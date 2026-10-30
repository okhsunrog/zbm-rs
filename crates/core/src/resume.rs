//! Boot-manager policy, not a parser or validator of Linux hibernation images.
use crate::boot::BootTarget;
use serde::{Deserialize, Serialize};
pub mod device;
pub mod linux;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Status {
    #[default]
    Inactive,
    Pending {
        session: Box<Session>,
    },
    Unavailable {
        session_id: String,
        reason: String,
    },
    Blocked {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub id: String,
    pub pool_guid: u64,
    pub target: BootTarget,
}

impl Session {
    /// Display/routing identity only. Linux handoff verifies actual artifacts.
    pub fn matches(&self, pool_guid: u64, target: &BootTarget) -> bool {
        let a = self.target.inputs();
        let b = target.inputs();
        self.pool_guid == pool_guid
            && self.target.snapshot.is_none()
            && target.snapshot.is_none()
            && self.target.same_identity(target)
            && a.initrd == b.initrd
            && a.init == b.init
            && a.kernel_params == b.kernel_params
    }
}

impl Status {
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::Pending { session } => Some(&session.id),
            Self::Unavailable { session_id, .. } => Some(session_id),
            _ => None,
        }
    }
    pub fn session(&self) -> Option<&Session> {
        match self {
            Self::Pending { session } => Some(session),
            _ => None,
        }
    }
    pub fn protected(&self) -> bool {
        !matches!(self, Self::Inactive)
    }
    pub fn matches(&self, pool: u64, target: &BootTarget) -> bool {
        self.session().is_some_and(|s| s.matches(pool, target))
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Pending { session }
                if session.id.is_empty()
                    || session.id.len() > 128
                    || !session
                        .id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
                    || session.pool_guid == 0
                    || session.target.snapshot.is_some() =>
            {
                Err("Invalid resume session identity")
            }
            Self::Blocked { reason } if reason.is_empty() || reason.len() > 4096 => {
                Err("Invalid resume failure description")
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::{Backend, NixosTarget};

    pub fn fixture() -> Session {
        Session {
            id: "session-1".into(),
            pool_guid: 42,
            target: BootTarget {
                dataset: "tank/root".into(),
                label: "NixOS".into(),
                root: "/run/root".into(),
                snapshot: None,
                mountpoint: None,
                backend: Backend::Nixos(NixosTarget {
                    generation: 2,
                    specialisation: None,
                    toplevel: "/nix/store/system".into(),
                    kernel: "/nix/store/kernel/bzImage".into(),
                    initrd: Some("/nix/store/initrd/initrd".into()),
                    init: "/nix/store/system/init".into(),
                    kernel_params: vec![],
                }),
            },
        }
    }

    #[test]
    fn shared_kernel_does_not_make_other_generation_or_variant_resumable() {
        let s = fixture();
        assert!(s.matches(42, &s.target));
        assert!(!s.matches(43, &s.target));
        let mut t = s.target.clone();
        if let Backend::Nixos(n) = &mut t.backend {
            n.specialisation = Some("other".into());
        }
        assert!(!s.matches(42, &t));
        let mut t = s.target.clone();
        if let Backend::Nixos(n) = &mut t.backend {
            n.initrd = Some("/changed".into());
        }
        assert!(!s.matches(42, &t));
        let mut t = s.target.clone();
        t.snapshot = Some(crate::boot::SnapshotSource {
            name: "tank/root@old".into(),
            guid: 3,
        });
        assert!(!s.matches(42, &t));
    }

    #[test]
    fn unknown_or_invalid_state_is_never_an_inactive_session() {
        assert!(serde_json::from_str::<Status>(r#"{"state":"unknown"}"#).is_err());
        let mut session = fixture();
        session.id.clear();
        let s = Status::Pending {
            session: Box::new(session),
        };
        assert!(s.validate().is_err());
        assert!(
            Status::Blocked {
                reason: "offline".into()
            }
            .protected()
        );
    }
}
