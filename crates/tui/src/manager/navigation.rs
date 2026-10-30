//! Selection and continuation across the optional navigation views.
use super::{Action, Options, Ui};
use zbm_core::State;

#[derive(Clone, Copy)]
pub(super) enum Intent {
    Menu,
    Boot,
    Kernels,
    Snapshots,
}
pub(super) struct UnlockSelection {
    pub(super) pool: zbm_core::Pool,
    pub(super) dataset: String,
    pub(super) intent: Intent,
    pub(super) challenge: Option<zbm_core::unlock::Challenge>,
}

pub(super) fn readonly(options: &Options) -> bool {
    matches!(
        options.config.zfs.import_policy,
        crate::config::ImportPolicy::ReadOnly
    )
}
pub(super) fn preferred(state: &State, options: &Options) -> Option<usize> {
    if let Some(session) = state.resume.session() {
        return state
            .environments
            .iter()
            .position(|e| e.pool_guid == session.pool_guid && e.dataset == session.target.dataset);
    }
    if let Some((guid, root)) = state.resume.unlock_target() {
        return state
            .environments
            .iter()
            .position(|e| e.pool_guid == guid && e.dataset == root);
    }
    if state.resume.protected() {
        return None;
    }
    zbm_core::environment::preferred_environment(
        &state.environments,
        options
            .config
            .boot
            .preferred_pool_guid
            .as_ref()
            .and_then(|guid| guid.parse().ok()),
    )
}
pub(super) fn merge(
    state: &mut State,
    guid: u64,
    environments: Vec<zbm_core::environment::BootEnvironment>,
    options: &Options,
) {
    let selected = state
        .environments
        .get(state.selected_environment)
        .map(|e| (e.pool_guid, e.dataset.clone()));
    state
        .environments
        .retain(|environment| environment.pool_guid != guid);
    state.environments.extend(environments);
    state.environments.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.dataset.cmp(&b.dataset))
            .then_with(|| a.pool_guid.cmp(&b.pool_guid))
    });
    state.selected_environment = selected
        .and_then(|(guid, dataset)| {
            state
                .environments
                .iter()
                .position(|e| e.pool_guid == guid && e.dataset == dataset)
        })
        .or_else(|| preferred(state, options))
        .unwrap_or(0);
}
pub(super) fn intent_action(
    intent: Intent,
    state: &mut State,
    _ui: &mut Ui,
    options: &Options,
    unlocking: &mut Option<UnlockSelection>,
) -> Option<Action> {
    if let Some((guid, root)) = state.resume.unlock_target() {
        let pool = state.pools.iter().find(|p| p.guid == guid)?.clone();
        let dataset = root.to_owned();
        *unlocking = Some(UnlockSelection {
            pool: pool.clone(),
            dataset: dataset.clone(),
            intent: Intent::Menu,
            challenge: None,
        });
        return Some(Action::Unlock {
            pool,
            dataset,
            answer: None,
            readonly: true,
            limit: options.config.nixos.generation_limit,
        });
    }
    let environment = state.environments.get(state.selected_environment)?.clone();
    let pool = state.environment_pool()?.clone();
    if environment.properties.locked()
        || state.resume.unlock_target() == Some((pool.guid, environment.dataset.as_str()))
    {
        *unlocking = Some(UnlockSelection {
            pool: pool.clone(),
            dataset: environment.dataset.clone(),
            intent,
            challenge: None,
        });
        return Some(Action::Unlock {
            pool,
            dataset: environment.dataset,
            answer: None,
            readonly: readonly(options),
            limit: options.config.nixos.generation_limit,
        });
    }
    match intent {
        Intent::Menu => None,
        Intent::Boot => {
            let target = if let Some(session) = state
                .resume
                .session()
                .filter(|s| s.pool_guid == pool.guid && s.target.dataset == environment.dataset)
            {
                environment
                    .targets
                    .iter()
                    .find(|t| session.matches(pool.guid, t))
            } else {
                environment
                    .default_target
                    .and_then(|index| environment.targets.get(index))
            };
            match target {
                Some(target) => Some(Action::Boot {
                    pool,
                    target: target.clone(),
                    args: options.config.kernel_args.clone(),
                }),
                None => {
                    state.error = environment
                        .default_error
                        .or(environment.unavailable)
                        .or(Some("No default boot target; choose one with K".into()));
                    None
                }
            }
        }
        Intent::Kernels => {
            state.pool_view = false;
            state.snapshot_view = false;
            state.targets = environment.targets;
            state.rejected_generations = environment.rejected_generations;
            state.selected_target = state
                .targets
                .iter()
                .position(|t| state.resume.matches(pool.guid, t))
                .or(environment.default_target)
                .unwrap_or(0);
            state.error = environment.unavailable;
            None
        }
        Intent::Snapshots => Some(Action::Snapshots {
            pool,
            dataset: environment.dataset,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernels_leave_pool_and_snapshot_views() {
        let environment = serde_json::from_value(serde_json::json!({
            "pool_guid": 42, "dataset": "tank/root", "is_default": true,
            "targets": [], "default_target": null, "default_error": null,
            "rejected_generations": [{"generation": 2, "error": "Invalid Bootspec"}],
            "unavailable": null, "root": null,
            "properties": {
                "mountpoint": "/", "canmount": "noauto", "encryption": "off",
                "encryptionroot": null, "keystatus": "-", "keyformat": "none",
                "active": "on", "commandline": null, "kernel": null, "rootprefix": null
            }
        }))
        .unwrap();
        let mut state = State {
            pool_view: true,
            snapshot_view: true,
            pools: vec![zbm_core::Pool {
                name: "tank".into(),
                guid: 42,
                health: "ONLINE".into(),
            }],
            environments: vec![environment],
            ..State::default()
        };
        let options = Options {
            preview: false,
            supervised: true,
            events: crate::diagnostics::Events::default(),
            config: crate::config::Config::default(),
        };
        let mut ui = Ui::default();
        ui.sync(&mut state);
        assert!(intent_action(Intent::Kernels, &mut state, &mut ui, &options, &mut None).is_none());
        ui.sync(&mut state);
        assert!(!state.pool_view);
        assert!(!state.snapshot_view);
        assert_eq!(ui.telemetry(&state)["view"], "Generations");
        assert_eq!(ui.telemetry(&state)["selected_row"]["Rejected"], 2);
    }
}
