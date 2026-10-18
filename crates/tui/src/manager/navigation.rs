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
    let environment = state.environments.get(state.selected_environment)?.clone();
    let pool = state.environment_pool()?.clone();
    if environment.properties.locked() {
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
            let target = environment
                .default_target
                .and_then(|index| environment.targets.get(index));
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
            state.targets = environment.targets;
            state.rejected_generations = environment.rejected_generations;
            state.selected_target = environment.default_target.unwrap_or(0);
            state.error = environment.unavailable;
            None
        }
        Intent::Snapshots => Some(Action::Snapshots {
            pool,
            dataset: environment.dataset,
        }),
    }
}
