//! Presentation and keyboard navigation. No filesystem reads or ZFS effects.
mod render;
use super::{
    Options,
    command::{ALL, Command},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
    widgets::ListState,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::Instant;
use zbm_core::{
    State,
    boot::{BootPlan, BootTarget},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum View {
    Pools,
    Environments,
    Snapshots,
    Generations,
    Kernels,
}

impl View {
    fn of(state: &State) -> Self {
        if state.pool_view {
            Self::Pools
        } else if !state.targets.is_empty() || !state.rejected_generations.is_empty() {
            if state
                .targets
                .first()
                .is_some_and(|t| t.generation().is_none())
            {
                Self::Kernels
            } else {
                Self::Generations
            }
        } else if state.snapshot_view {
            Self::Snapshots
        } else if !state.environments.is_empty() {
            Self::Environments
        } else {
            Self::Pools
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Pools => "ZFS pools",
            Self::Environments => "Boot environments (* bootfs)",
            Self::Snapshots => "Snapshots (newest first)",
            Self::Generations => "NixOS generations",
            Self::Kernels => "Linux kernels",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
enum RowId {
    Pool(u64),
    Environment(String),
    Snapshot(u64),
    Generation(u64),
    Specialisation(u64, String),
    Kernel(String),
    Rejected(u64),
    RejectedSpecialisation(u64, String),
}
#[derive(Clone, Copy)]
enum Kind {
    Pool(usize),
    Environment(usize),
    Snapshot(usize),
    Generation(usize),
    Rejected(usize),
}
struct Row {
    id: RowId,
    kind: Kind,
    title: String,
    subtitle: String,
    tag: &'static str,
}
#[derive(Default)]
struct Screen {
    query: String,
    selected: Option<RowId>,
    list: ListState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub enum Panel {
    #[default]
    None,
    Help,
    Actions,
    Details,
    ConfirmDiscard,
    ConfirmRollback,
    ConfirmPromote,
    ConfirmResume,
    Unlock,
}

/// Presentation state only; does not authorize boot or alter cached ZFS facts.
pub struct UnlockProgress {
    pub pool_guid: u64,
    pub dataset: String,
    pub discovering: bool,
}

#[derive(Default)]
pub struct Ui {
    pub unlock_progress: Option<UnlockProgress>,
    pub resume_confirmation: Option<(String, Command)>,
    pub unlock_prompt: Option<String>,
    screens: BTreeMap<String, Screen>,
    key: String,
    pub searching: bool,
    resume_search: bool,
    pub panel: Panel,
    menu: ListState,
    menu_view: ListState,
    menu_commands: Vec<Command>,
    scroll: u16,
    scroll_max: u16,
    page_size: usize,
    pub prepared: Option<BootPlan>,
    pub operation_started: Option<Instant>,
    confirmation: String,
    passphrase: zeroize::Zeroizing<String>,
}

pub enum Input {
    Unlock(zeroize::Zeroizing<Vec<u8>>),
    Changed(&'static str),
    Command(Command, bool),
    Ignored,
}

impl Ui {
    fn unlock_status(&self, pool_guid: u64, dataset: &str) -> Option<&'static str> {
        self.unlock_progress
            .as_ref()
            .filter(|p| p.pool_guid == pool_guid && p.dataset == dataset)
            .map(|p| {
                if p.discovering {
                    "Keys unlocked; finding boot targets…"
                } else {
                    "Unlocking encryption keys…"
                }
            })
    }
}

fn target_id(t: &BootTarget) -> RowId {
    t.generation()
        .map(|g| match t.specialisation() {
            Some(name) => RowId::Specialisation(g, name.into()),
            None => RowId::Generation(g),
        })
        .unwrap_or_else(|| RowId::Kernel(t.inputs().kernel.display().to_string()))
}

fn variant_title(generation: u64, variant: Option<&str>) -> String {
    match variant {
        Some(name) => format!("Generation {generation} · {}", clean(name)),
        None => format!("Generation {generation}"),
    }
}

fn clean(value: impl AsRef<str>) -> String {
    value
        .as_ref()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
fn snapshot_date(seconds: u64) -> String {
    let date = i64::try_from(seconds)
        .ok()
        .and_then(|seconds| time::OffsetDateTime::from_unix_timestamp(seconds).ok());
    match date {
        Some(date) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02} UTC",
            date.year(),
            date.month() as u8,
            date.day(),
            date.hour(),
            date.minute()
        ),
        None => "Unknown date".into(),
    }
}

fn fuzzy(value: &str, query: &str) -> bool {
    let mut chars = value.chars().flat_map(char::to_lowercase);
    query
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !c.is_whitespace())
        .all(|needle| chars.any(|c| c == needle))
}

impl Ui {
    pub fn focus_environment(&mut self, state: &mut State, index: usize) {
        state.selected_environment = index;
        if let Some(environment) = state.environments.get(index)
            && let Some(screen) = self.screens.get_mut("environments")
        {
            screen.selected = Some(RowId::Environment(format!(
                "{}:{}",
                environment.pool_guid, environment.dataset
            )));
        }
    }
    pub fn reset_after_mutation(&mut self) {
        use zeroize::Zeroize;
        self.passphrase.zeroize();
        self.screens.clear();
        self.key.clear();
        self.panel = Panel::None;
        self.searching = false;
        self.resume_search = false;
        self.prepared = None;
    }
    fn rows(&self, state: &State) -> Vec<Row> {
        match View::of(state) {
            View::Pools => state
                .pools
                .iter()
                .enumerate()
                .map(|(i, p)| Row {
                    id: RowId::Pool(p.guid),
                    kind: Kind::Pool(i),
                    title: p.name.clone(),
                    subtitle: state
                        .pool_errors
                        .get(&p.guid)
                        .cloned()
                        .unwrap_or_else(|| p.health.clone()),
                    tag: "POOL",
                })
                .collect(),
            View::Environments => state
                .environments
                .iter()
                .enumerate()
                .map(|(i, e)| Row {
                    id: RowId::Environment(format!("{}:{}", e.pool_guid, e.dataset)),
                    kind: Kind::Environment(i),
                    title: format!("{}{}", if e.is_default { "* " } else { "" }, e.dataset),
                    subtitle: self
                        .unlock_status(e.pool_guid, &e.dataset)
                        .map(str::to_owned)
                        .or_else(|| e.unavailable.clone())
                        .or_else(|| e.default_error.clone())
                        .unwrap_or_else(|| {
                            e.targets
                                .iter()
                                .find(|target| state.resume.matches(e.pool_guid, target))
                                .or_else(|| e.default_target.and_then(|index| e.targets.get(index)))
                                .map(|target| {
                                    target
                                        .generation()
                                        .map(|number| {
                                            variant_title(number, target.specialisation())
                                        })
                                        .unwrap_or_else(|| target.label.clone())
                                })
                                .unwrap_or_else(|| {
                                    "No default boot target; K chooses an alternative".into()
                                })
                        }),
                    tag: if let Some(progress) = self
                        .unlock_progress
                        .as_ref()
                        .filter(|p| p.pool_guid == e.pool_guid && p.dataset == e.dataset)
                    {
                        if progress.discovering {
                            "DISCOVERING"
                        } else {
                            "UNLOCKING"
                        }
                    } else if e.is_default && e.unavailable.is_some() {
                        if e.properties.locked() {
                            "BOOTFS / LOCKED"
                        } else {
                            "BOOTFS / ERROR"
                        }
                    } else if e.unavailable.is_some() {
                        if e.properties.locked() {
                            "LOCKED"
                        } else {
                            "ERROR"
                        }
                    } else if e.is_default {
                        "BOOTFS"
                    } else {
                        "BE"
                    },
                })
                .collect(),
            View::Snapshots => state
                .snapshots
                .iter()
                .enumerate()
                .map(|(i, s)| Row {
                    id: RowId::Snapshot(s.source.guid),
                    kind: Kind::Snapshot(i),
                    title: s.source.name.clone(),
                    subtitle: format!("Created {}", snapshot_date(s.creation)),
                    tag: "SNAPSHOT",
                })
                .collect(),
            View::Generations | View::Kernels => {
                let mut rows: Vec<_> = state
                    .targets
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        (
                            t.generation().unwrap_or(0),
                            Row {
                                id: target_id(t),
                                kind: Kind::Generation(i),
                                title: t
                                    .generation()
                                    .map(|g| variant_title(g, t.specialisation()))
                                    .unwrap_or_else(|| t.label.clone()),
                                subtitle: t.issue().map(str::to_owned).unwrap_or_else(|| {
                                    if t.generation().is_some() {
                                        t.label.clone()
                                    } else {
                                        t.inputs()
                                            .initrd
                                            .as_ref()
                                            .map(|p| p.display().to_string())
                                            .unwrap_or_default()
                                    }
                                }),
                                tag: if t.issue().is_some() {
                                    "UNAVAILABLE"
                                } else if t.snapshot.is_some() {
                                    "SNAPSHOT"
                                } else {
                                    if t.generation().is_some() {
                                        "NIXOS"
                                    } else {
                                        "KERNEL"
                                    }
                                },
                            },
                        )
                    })
                    .chain(state.rejected_generations.iter().enumerate().map(|(i, e)| {
                        (
                            e.generation,
                            Row {
                                id: match &e.specialisation {
                                    Some(name) => {
                                        RowId::RejectedSpecialisation(e.generation, name.clone())
                                    }
                                    None => RowId::Rejected(e.generation),
                                },
                                kind: Kind::Rejected(i),
                                title: variant_title(e.generation, e.specialisation.as_deref()),
                                subtitle: e.error.clone(),
                                tag: "UNAVAILABLE",
                            },
                        )
                    }))
                    .collect();
                rows.sort_by_key(|(generation, _)| std::cmp::Reverse(*generation));
                rows.into_iter().map(|(_, row)| row).collect()
            }
        }
    }

    fn visible(&self, state: &State) -> Vec<Row> {
        let query = &self.screens[&self.key].query;
        self.rows(state)
            .into_iter()
            .filter(|row| fuzzy(&format!("{} {}", row.title, row.subtitle), query))
            .collect()
    }
    fn selected(&self, state: &State) -> Option<Row> {
        let selected = self.screens.get(&self.key)?.selected.as_ref()?;
        self.visible(state)
            .into_iter()
            .find(|row| &row.id == selected)
    }
    pub fn target<'a>(&self, state: &'a State) -> Option<&'a BootTarget> {
        match self.selected(state)?.kind {
            Kind::Generation(i) => state.targets.get(i),
            _ => None,
        }
    }
    /// Resolve the cached boot selection before invalidating its saved session.
    pub fn boot_target<'a>(&self, state: &'a State) -> Option<&'a BootTarget> {
        match self.selected(state)?.kind {
            Kind::Generation(i) => state.targets.get(i),
            Kind::Environment(i) => {
                let environment = state.environments.get(i)?;
                if environment.properties.locked() || environment.unavailable.is_some() {
                    return None;
                }
                environment
                    .targets
                    .iter()
                    .find(|target| state.resume.matches(environment.pool_guid, target))
                    .or_else(|| {
                        environment
                            .default_target
                            .and_then(|index| environment.targets.get(index))
                    })
            }
            _ => None,
        }
    }
    pub fn resume_selected(&self, state: &State) -> bool {
        match self.selected(state).map(|r| r.kind) {
            Some(Kind::Environment(i)) => {
                let e = &state.environments[i];
                !e.properties.locked()
                    && e.targets
                        .iter()
                        .any(|t| state.resume.matches(e.pool_guid, t))
            }
            Some(Kind::Generation(i)) => state
                .pools
                .get(state.selected)
                .is_some_and(|p| state.resume.matches(p.guid, &state.targets[i])),
            _ => false,
        }
    }
    pub fn cancels_resume(&self, command: Command, state: &State) -> bool {
        if state.resume.session_id().is_none() {
            return false;
        }
        match command {
            Command::Open => match self.selected(state).map(|r| r.kind) {
                Some(Kind::Environment(i)) => {
                    let e = &state.environments[i];
                    !state.resume.session().is_some_and(|session| {
                        e.pool_guid == session.pool_guid && e.dataset == session.target.dataset
                    })
                }
                Some(Kind::Generation(_)) => !self.resume_selected(state),
                _ => false,
            },
            Command::Prepare
            | Command::CancelResume
            | Command::ColdBoot
            | Command::Discard
            | Command::Clone
            | Command::ClonePromote
            | Command::Rollback
            | Command::Shell => true,
            _ => false,
        }
    }
    pub fn confirm_resume(&mut self, command: Command, state: &State) {
        let panel = match command {
            Command::Discard => Panel::ConfirmDiscard,
            Command::ClonePromote => Panel::ConfirmPromote,
            Command::Rollback => Panel::ConfirmRollback,
            _ => Panel::ConfirmResume,
        };
        self.open_panel(panel);
        self.resume_confirmation = state.resume.session_id().map(|id| (id.into(), command));
    }
    pub fn sync(&mut self, state: &mut State) {
        if state.activity.operation().is_none() {
            self.operation_started = None;
        }
        let view = View::of(state);
        let pool = state.pools.get(state.selected).map(|p| p.guid).unwrap_or(0);
        let environment = state
            .environments
            .get(state.selected_environment)
            .map(|e| e.dataset.as_str())
            .unwrap_or("");
        let snapshot = if state.snapshot_view && matches!(view, View::Generations | View::Kernels) {
            state
                .snapshots
                .get(state.selected_snapshot)
                .map(|s| s.source.guid)
                .unwrap_or(0)
        } else {
            0
        };
        let key = match view {
            View::Pools => "pools".into(),
            View::Environments => "environments".into(),
            View::Snapshots => format!("{pool}/{environment}/snapshots"),
            View::Generations | View::Kernels => {
                format!("{pool}/{environment}/{snapshot}/generations")
            }
        };
        if self.key != key {
            if crate::diagnostics::continuous() {
                eprintln!(
                    "[{}] ui-view-change previous-panel={:?} next-view={view:?}",
                    crate::diagnostics::timestamp(),
                    self.panel
                );
            }
            self.searching = false;
            self.resume_search = false;
            self.panel = Panel::None;
            self.scroll = 0;
            self.key = key;
        }
        let initial = match view {
            View::Pools => state.pools.get(state.selected).map(|p| RowId::Pool(p.guid)),
            View::Environments => state
                .environments
                .get(state.selected_environment)
                .map(|e| RowId::Environment(format!("{}:{}", e.pool_guid, e.dataset))),
            View::Snapshots => state
                .snapshots
                .get(state.selected_snapshot)
                .map(|s| RowId::Snapshot(s.source.guid)),
            View::Generations | View::Kernels => {
                state.targets.get(state.selected_target).map(target_id)
            }
        };
        self.screens
            .entry(self.key.clone())
            .or_insert_with(|| Screen {
                selected: initial,
                ..Screen::default()
            });
        let rows = self.visible(state);
        let screen = self.screens.get_mut(&self.key).unwrap();
        if !rows
            .iter()
            .any(|row| Some(&row.id) == screen.selected.as_ref())
        {
            screen.selected = rows.first().map(|row| row.id.clone());
        }
        screen.list.select(
            rows.iter()
                .position(|row| Some(&row.id) == screen.selected.as_ref()),
        );
        if let Some(row) = self.selected(state) {
            match row.kind {
                Kind::Pool(i) => state.selected = i,
                Kind::Environment(i) => {
                    state.selected_environment = i;
                    if let Some(pool) = state
                        .pools
                        .iter()
                        .position(|pool| pool.guid == state.environments[i].pool_guid)
                    {
                        state.selected = pool;
                    }
                }
                Kind::Snapshot(i) => state.selected_snapshot = i,
                Kind::Generation(i) => state.selected_target = i,
                Kind::Rejected(_) => {}
            }
        }
    }
    pub fn telemetry(&self, state: &State) -> serde_json::Value {
        serde_json::json!({"unlock_progress":self.unlock_progress.as_ref().map(|p| if p.discovering { "discovering" } else { "unlocking" }),"view":View::of(state),"filter":self.screens.get(&self.key).map(|s|s.query.as_str()).unwrap_or(""),"searching":self.searching,"panel":self.panel,"selected_row":self.selected(state).map(|r|r.id),"selected_action":if self.panel == Panel::Actions { self.menu.selected().and_then(|i| self.menu_commands.get(i)) } else { None }})
    }
    pub fn reason(&self, command: Command, state: &State, options: &Options) -> Option<String> {
        if options.config.security.mode == crate::config::SecurityMode::Enforce
            && matches!(
                command,
                Command::Rollback | Command::Clone | Command::ClonePromote
            )
        {
            return Some(
                "Protected mode requires separate owner authorization for administrative mutations"
                    .into(),
            );
        }
        if command == Command::Shell
            && options.config.security.mode == crate::config::SecurityMode::Enforce
        {
            return Some("Protected recovery does not allow an administrative shell".into());
        }
        if matches!(
            command,
            Command::Help | Command::Actions | Command::Details | Command::Exit
        ) {
            return None;
        }
        if matches!(command, Command::Restart | Command::PowerOff) {
            return (!options.supervised).then(|| "Requires the PID 1 supervisor".into());
        }
        if let zbm_core::resume::Status::Blocked { reason } = &state.resume
            && !matches!(
                command,
                Command::Search | Command::Back | Command::Rescan | Command::Pools
            )
        {
            return Some(format!(
                "Saved session could not be checked: {}",
                clean(reason)
            ));
        }
        if command == Command::Shell && !state.resume.protected() {
            return None;
        }
        if state.activity.requires_restart() {
            return Some("Restart manager to reconcile the timed-out operation".into());
        }
        if state.activity.operation().is_some() || state.activity.is_scanning() {
            return Some(
                "An operation is in progress; Restart and Power off remain available".into(),
            );
        }
        if state.resume.unlock_target().is_some() {
            return match command {
                Command::Open
                | Command::Search
                | Command::Back
                | Command::Rescan
                | Command::Pools => None,
                _ => Some("Unlock encrypted swap before further boot actions".into()),
            };
        }
        if matches!(
            command,
            Command::Search | Command::Back | Command::Rescan | Command::Pools | Command::Shell
        ) {
            return None;
        }
        if !options.supervised || options.preview {
            return Some("Boot actions are unavailable in local preview".into());
        }
        if command.needs_saved_session() {
            if state.resume.session_id().is_none() {
                return Some("No saved session to discard".into());
            }
            if command == Command::CancelResume {
                return None;
            }
            return self.reason(Command::Open, state, options).or_else(|| {
                self.boot_target(state)
                    .is_none()
                    .then(|| "Select a usable system, kernel or generation first".into())
            });
        }
        if command == Command::Kernels {
            return state
                .environments
                .get(state.selected_environment)
                .and_then(|environment| {
                    if environment.properties.locked() {
                        None
                    } else {
                        environment.unavailable.clone()
                    }
                })
                .or_else(|| {
                    state
                        .environments
                        .is_empty()
                        .then(|| "Select a boot environment first".into())
                });
        }
        if matches!(
            command,
            Command::Clone | Command::ClonePromote | Command::Rollback
        ) {
            if !state.snapshot_view || state.snapshots.get(state.selected_snapshot).is_none() {
                return Some("Select a snapshot first".into());
            }
            if matches!(
                options.config.zfs.import_policy,
                crate::config::ImportPolicy::ReadOnly
            ) {
                return Some("Image import policy is read-only".into());
            }
            return None;
        }
        if command == Command::Snapshots {
            if !options.config.ui.show_snapshots {
                return Some("Snapshots are disabled by image configuration".into());
            }
            return match state.environments.get(state.selected_environment) {
                Some(_) => None,
                None => Some("Select a boot environment first".into()),
            };
        }
        if command == Command::Discard {
            if !state.snapshot_view || state.snapshots.get(state.selected_snapshot).is_none() {
                return Some("Select a snapshot first".into());
            }
            return matches!(
                options.config.zfs.import_policy,
                crate::config::ImportPolicy::ReadOnly
            )
            .then(|| "Image import policy is read-only".into());
        }
        if command == Command::Prepare {
            if let Some(issue) = self
                .target(state)
                .and_then(|t| t.issue().map(str::to_owned))
            {
                return Some(issue);
            }
            if !state.snapshot_view
                || self
                    .target(state)
                    .and_then(|target| target.snapshot.as_ref())
                    .is_none()
            {
                return Some("Select a usable snapshot generation first".into());
            }
            if matches!(
                options.config.zfs.import_policy,
                crate::config::ImportPolicy::ReadOnly
            ) {
                return Some(
                    "The image import policy is read-only; clone mutations are unavailable".into(),
                );
            }
            return None;
        }
        if let Some(issue) = self
            .target(state)
            .and_then(|t| t.issue().map(str::to_owned))
        {
            return Some(issue);
        }
        match self.selected(state).map(|row| row.kind) {
            None => Some("No selectable target matches this filter".into()),
            Some(Kind::Rejected(i)) => Some(format!(
                "generation {}: {}",
                state.rejected_generations[i].generation, state.rejected_generations[i].error
            )),
            Some(Kind::Environment(i)) if state.environments[i].properties.locked() => None,
            Some(Kind::Environment(i)) if state.environments[i].root.is_none() => state
                .environments[i]
                .unavailable
                .clone()
                .or(Some("Boot environment is unavailable".into())),
            _ => None,
        }
    }
    pub fn open_label(&self, state: &State) -> &'static str {
        if state.resume.unlock_target().is_some() {
            return "Unlock swap";
        }
        if self.resume_selected(state) {
            return "Resume";
        }
        match View::of(state) {
            View::Pools => "Import",
            View::Environments
                if state
                    .environments
                    .get(state.selected_environment)
                    .is_some_and(|e| e.properties.locked()) =>
            {
                "Unlock"
            }
            View::Environments => "Boot",
            View::Snapshots => "Open",
            View::Generations | View::Kernels if state.snapshot_view => "Boot clone",
            View::Generations | View::Kernels => "Boot",
        }
    }
    pub fn open_panel(&mut self, panel: Panel) {
        use zeroize::Zeroize;
        self.passphrase.zeroize();
        if panel == Panel::Unlock && self.passphrase.capacity() < 1024 {
            // Reserve before the first secret byte, avoiding growth copies.
            self.passphrase.reserve_exact(1024);
        }
        self.confirmation.clear();
        self.resume_confirmation = None;
        if self.panel == Panel::None && self.searching {
            self.resume_search = true;
        }
        self.searching = false;
        self.panel = panel;
        self.scroll = 0;
        self.scroll_max = 0;
        self.menu_commands = ALL.into_iter().filter(|c| *c != Command::Actions).collect();
        self.menu.select(Some(0));
        self.menu_view = ListState::default();
    }
    pub fn input(&mut self, key: KeyEvent, state: &mut State) -> Input {
        if crate::diagnostics::continuous()
            && matches!(key.code, KeyCode::F(_) | KeyCode::Esc | KeyCode::Enter)
        {
            // Never format Char, Backspace, the secret buffer or its length.
            eprintln!(
                "[{}] ui-control={:?} modifiers={:?} panel={:?}",
                crate::diagnostics::timestamp(),
                key.code,
                key.modifiers,
                self.panel
            );
        }
        let command = Command::from_key(key);
        if self.panel == Panel::Unlock {
            use zeroize::Zeroize;
            match key.code {
                KeyCode::Esc => {
                    self.passphrase.zeroize();
                    self.panel = Panel::None;
                    return Input::Changed("unlock-cancelled");
                }
                KeyCode::Enter if command == Some(Command::Open) && !self.passphrase.is_empty() => {
                    let bytes = zeroize::Zeroizing::new(self.passphrase.as_bytes().to_vec());
                    self.passphrase.zeroize();
                    self.panel = Panel::None;
                    return Input::Unlock(bytes);
                }
                KeyCode::Backspace => {
                    // Clear the removed character too, including the allocation's tail.
                    if let Some((index, _)) = self.passphrase.char_indices().next_back() {
                        let prefix = zeroize::Zeroizing::new(self.passphrase[..index].to_owned());
                        self.passphrase.zeroize();
                        self.passphrase.push_str(&prefix);
                    }
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && key.modifiers.difference(KeyModifiers::SHIFT).is_empty() =>
                {
                    if self.passphrase.len() + c.len_utf8() <= 1024 {
                        self.passphrase.push(c);
                    }
                }
                KeyCode::F(_) if command.is_some_and(Command::recovery) => {
                    self.passphrase.zeroize();
                    self.panel = Panel::None;
                    return Input::Command(command.unwrap(), false);
                }
                _ => return Input::Ignored,
            }
            // Do not emit a telemetry record for each secret keystroke.
            return Input::Ignored;
        }
        if matches!(key.code, KeyCode::F(_))
            && let Some(command) = command
            && (self.panel == Panel::None || command.recovery())
        {
            return Input::Command(command, false);
        }
        if self.searching {
            if state.activity.operation().is_some()
                || state.activity.is_scanning()
                || state.activity.requires_restart()
            {
                return Input::Ignored;
            }
            let screen = self.screens.get_mut(&self.key).unwrap();
            match key.code {
                KeyCode::Esc => {
                    screen.query.clear();
                    self.searching = false;
                }
                KeyCode::Enter => self.searching = false,
                KeyCode::Backspace => {
                    screen.query.pop();
                }
                KeyCode::Char(c)
                    if !key.modifiers.intersects(
                        KeyModifiers::CONTROL
                            | KeyModifiers::ALT
                            | KeyModifiers::SUPER
                            | KeyModifiers::META
                            | KeyModifiers::HYPER,
                    ) =>
                {
                    screen.query.push(c);
                }
                _ => return Input::Ignored,
            }
            self.sync(state);
            return Input::Changed("filter");
        }
        if self.panel == Panel::ConfirmRollback {
            match key.code {
                KeyCode::Esc => {
                    self.panel = Panel::None;
                    self.confirmation.clear();
                    return Input::Changed("panel-closed");
                }
                KeyCode::Enter
                    if command == Some(Command::Open) && self.confirmation == "ROLLBACK" =>
                {
                    self.panel = Panel::None;
                    return Input::Command(Command::Rollback, true);
                }
                KeyCode::Backspace => {
                    self.confirmation.pop();
                    return Input::Changed("rollback-confirmation");
                }
                KeyCode::Char(c)
                    if !key.modifiers.intersects(
                        KeyModifiers::CONTROL
                            | KeyModifiers::ALT
                            | KeyModifiers::SUPER
                            | KeyModifiers::META
                            | KeyModifiers::HYPER,
                    ) =>
                {
                    if self.confirmation.len() < 32 {
                        self.confirmation.push(c);
                    }
                    return Input::Changed("rollback-confirmation");
                }
                KeyCode::F(_) => {}
                _ => return Input::Ignored,
            }
        }
        if self.panel == Panel::None && command.is_some_and(Command::recovery) {
            return Input::Command(command.unwrap(), false);
        }
        if self.panel != Panel::None {
            if key.code == KeyCode::Esc {
                self.resume_confirmation = None;
                self.panel = Panel::None;
                self.searching = std::mem::take(&mut self.resume_search);
                return Input::Changed("panel-closed");
            }
            if self.panel == Panel::ConfirmResume {
                if command == Some(Command::Open)
                    && let Some((_, pending)) = self.resume_confirmation.as_ref()
                {
                    let pending = *pending;
                    self.panel = Panel::None;
                    return Input::Command(pending, true);
                }
                return Input::Ignored;
            }
            if self.panel == Panel::ConfirmDiscard {
                if command == Some(Command::Open) {
                    self.panel = Panel::None;
                    self.resume_search = false;
                    return Input::Command(Command::Discard, true);
                }
                return Input::Ignored;
            }
            if self.panel == Panel::ConfirmPromote {
                if command == Some(Command::Open) {
                    self.panel = Panel::None;
                    return Input::Command(Command::ClonePromote, true);
                }
                return Input::Ignored;
            }
            if self.panel == Panel::Actions {
                match key.code {
                    KeyCode::Up => self.menu.select(Some(
                        (self.menu.selected().unwrap_or(0) + self.menu_commands.len() - 1)
                            % self.menu_commands.len(),
                    )),
                    KeyCode::Down => self.menu.select(Some(
                        (self.menu.selected().unwrap_or(0) + 1) % self.menu_commands.len(),
                    )),
                    KeyCode::Home => self.menu.select(Some(0)),
                    KeyCode::End => self.menu.select(Some(self.menu_commands.len() - 1)),
                    KeyCode::PageDown => self.menu.select(Some(
                        (self.menu.selected().unwrap_or(0) + 5).min(self.menu_commands.len() - 1),
                    )),
                    KeyCode::PageUp => self
                        .menu
                        .select(Some(self.menu.selected().unwrap_or(0).saturating_sub(5))),
                    KeyCode::Enter if command == Some(Command::Open) => {
                        let command = self.menu_commands[self.menu.selected().unwrap_or(0)];
                        self.panel = Panel::None;
                        self.resume_search = false;
                        return Input::Command(command, false);
                    }
                    _ => {
                        if let Some(command) = command {
                            self.panel = Panel::None;
                            self.resume_search = false;
                            return Input::Command(command, false);
                        } else {
                            return Input::Ignored;
                        }
                    }
                }
            } else {
                match key.code {
                    KeyCode::Down => {
                        self.scroll = self.scroll.saturating_add(1).min(self.scroll_max)
                    }
                    KeyCode::PageDown => {
                        self.scroll = self.scroll.saturating_add(8).min(self.scroll_max)
                    }
                    KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
                    KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(8),
                    KeyCode::Home => self.scroll = 0,
                    KeyCode::End => self.scroll = self.scroll_max,
                    _ => return Input::Ignored,
                }
            }
            return Input::Changed("panel");
        }
        if key.code == KeyCode::Esc && !self.screens[&self.key].query.is_empty() {
            self.screens.get_mut(&self.key).unwrap().query.clear();
            self.sync(state);
            return Input::Changed("filter");
        }
        if state.activity.operation().is_none()
            && !state.activity.requires_restart()
            && !state.activity.is_scanning()
        {
            let rows = self.visible(state);
            let screen = self.screens.get_mut(&self.key).unwrap();
            let current = screen.list.selected().unwrap_or(0);
            let next = match key.code {
                KeyCode::Up => Some((current + rows.len().saturating_sub(1)) % rows.len().max(1)),
                KeyCode::Down => Some((current + 1) % rows.len().max(1)),
                KeyCode::Home => Some(0),
                KeyCode::End => Some(rows.len().saturating_sub(1)),
                KeyCode::PageDown => {
                    Some((current + self.page_size.max(1)).min(rows.len().saturating_sub(1)))
                }
                KeyCode::PageUp => Some(current.saturating_sub(self.page_size.max(1))),
                _ => None,
            };
            if let Some(next) = next {
                screen.selected = rows.get(next).map(|row| row.id.clone());
                self.sync(state);
                return Input::Changed("selection");
            }
        }
        command
            .map(|command| Input::Command(command, false))
            .unwrap_or(Input::Ignored)
    }
    fn prepared_target(&self, target: &BootTarget) -> Option<&BootPlan> {
        self.prepared.as_ref().filter(|plan| {
            plan.target.generation() == target.generation()
                && plan.target.specialisation() == target.specialisation()
                && plan.target.toplevel() == target.toplevel()
                && plan.target.inputs().kernel == target.inputs().kernel
                && plan
                    .target
                    .snapshot
                    .as_ref()
                    .zip(target.snapshot.as_ref())
                    .is_some_and(|(a, b)| a.guid == b.guid && a.name == b.name)
        })
    }

    fn details(&self, state: &State, options: &Options) -> Vec<Line<'static>> {
        let mut lines = vec![];
        let mut field = |key: &str, value: String| {
            lines.push(Line::from(vec![
                Span::styled(format!("{key}: "), Style::default().fg(Color::Cyan)),
                Span::raw(clean(value)),
            ]));
        };
        if let Some(error) = &state.error {
            field("Notice", error.clone());
        }
        match &state.resume {
            zbm_core::resume::Status::Locked {
                encryption_root, ..
            } => {
                field(
                    "Saved session",
                    format!("Unlock {encryption_root} and encrypted swap before checking resume"),
                );
            }
            zbm_core::resume::Status::Pending { session } => {
                field(
                    "Saved session",
                    format!("{}: {}", session.target.dataset, session.target.label),
                );
                field(
                    "Resume",
                    "The matching target resumes; other boot actions require cancellation.".into(),
                );
            }
            zbm_core::resume::Status::Unavailable { reason, .. }
            | zbm_core::resume::Status::Blocked { reason } => {
                field("Resume unavailable", reason.clone())
            }
            _ => (),
        }
        match self.selected(state).map(|row| row.kind) {
            Some(Kind::Pool(i)) => {
                let p = &state.pools[i];
                field("Pool", p.name.clone());
                field("GUID", p.guid.to_string());
                field("Health", p.health.clone());
                field(
                    "Import policy",
                    match options.config.zfs.import_policy {
                        crate::config::ImportPolicy::ReadOnly => "Read-only",
                        crate::config::ImportPolicy::HostId => "Host ID guarded",
                    }
                    .into(),
                );
                field("Action", "Enter explicitly imports without force".into());
            }
            Some(Kind::Environment(i)) => {
                let e = &state.environments[i];
                field("Dataset", e.dataset.clone());
                field(
                    "Default",
                    if e.is_default { "Pool bootfs" } else { "No" }.into(),
                );
                field("Mountpoint", e.properties.mountpoint.clone());
                field("Canmount", e.properties.canmount.clone());
                field("Encryption", e.properties.encryption.clone());
                field(
                    "Inspection",
                    if e.root.is_some() {
                        "Read-only mount"
                    } else {
                        "Not mounted"
                    }
                    .into(),
                );
                field(
                    "Boot targets",
                    format!(
                        "{} usable / {} unavailable",
                        e.targets.len(),
                        e.rejected_generations.len()
                    ),
                );
                if let Some(error) = &e.unavailable {
                    field("Unavailable", error.clone());
                }
            }
            Some(Kind::Snapshot(i)) => {
                let s = &state.snapshots[i];
                field("Source", s.source.name.clone());
                field("GUID", s.source.guid.to_string());
                field("Created", snapshot_date(s.creation));
                field("Creation (Unix)", s.creation.to_string());
                field(
                    "Action",
                    "Inspect boot targets, clone a BE, or roll back the dataset".into(),
                );
            }
            Some(Kind::Rejected(i)) => {
                let e = &state.rejected_generations[i];
                field("Generation", e.generation.to_string());
                if let Some(name) = &e.specialisation {
                    field("Variant", clean(name));
                }
                field("Unavailable", e.error.clone());
                field("Boot", "Blocked; usable peers remain available".into());
            }
            Some(Kind::Generation(i)) => {
                let t = &state.targets[i];
                if let Some(issue) = &t.issue() {
                    field("Unavailable", issue.to_string());
                }
                if let Some(g) = t.generation() {
                    field("Generation", g.to_string());
                    if let Some(name) = t.specialisation() {
                        field("Variant", clean(name));
                    }
                }
                field("Label", t.label.clone());
                field("Dataset", t.dataset.clone());
                if let Some(p) = &t.toplevel() {
                    field("Toplevel", p.display().to_string());
                }
                field("Kernel", t.inputs().kernel.display().to_string());
                field(
                    "Initrd",
                    t.inputs()
                        .initrd
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or("None".into()),
                );
                if let Some(p) = &t.inputs().init {
                    field("Init", p.display().to_string());
                }
                if let Some(s) = &t.snapshot {
                    field("Snapshot", s.name.clone());
                    field("Snapshot GUID", s.guid.to_string());
                    field("Root override", "Validated during clone preparation".into());
                }
                let plan = self.prepared_target(t);
                if let Some(plan) = plan {
                    field("Prepared clone", plan.target.dataset.clone());
                    field("Prepared cmdline", plan.cmdline.join(" "));
                } else {
                    let mut args = t.inputs().command_line(&options.config.kernel_args);
                    if let zbm_core::boot::Backend::Linux(linux) = &t.backend {
                        args.retain(|arg| !arg.starts_with("root=") && !arg.starts_with("zfs="));
                        args.push(format!("{}{}", linux.rootprefix, t.dataset));
                    }
                    field(
                        if t.generation().is_some() {
                            "Bootspec cmdline"
                        } else {
                            "Kernel cmdline"
                        },
                        zbm_core::linux::command_line(&args)
                            .unwrap_or_else(|_| "Invalid command line".into()),
                    );
                }
                field("Validation", "Execution revalidates all boot inputs".into());
            }
            None => field(
                "Selection",
                "No matching targets. / searches; Esc clears the filter.".into(),
            ),
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn resume_state() -> State {
        let target: BootTarget = serde_json::from_value(serde_json::json!({
            "dataset":"tank/root","generation":7,"label":"NixOS","root":"/guest",
            "toplevel":"/nix/store/base","backend":{"kind":"Nixos"},"snapshot":null,
            "mountpoint":null,"issue":null,
            "inputs":{"kernel":"/shared/kernel","initrd":null,"init":"/nix/store/base/init","kernel_params":[]}
        })).unwrap();
        let mut other = target.clone();
        if let zbm_core::boot::Backend::Nixos(n) = &mut other.backend {
            n.generation = 6;
        }
        State {
            resume: zbm_core::resume::Status::Pending {
                session: Box::new(zbm_core::resume::Session {
                    id: "session-one".into(),
                    pool_guid: 42,
                    target: target.clone(),
                }),
            },
            pools: vec![zbm_core::Pool {
                name: "tank".into(),
                guid: 42,
                health: "ONLINE".into(),
            }],
            targets: vec![target, other],
            ..State::default()
        }
    }
    #[test]
    fn resume_navigation_does_not_cancel_or_add_a_startup_panel() {
        let mut state = resume_state();
        let mut ui = Ui::default();
        ui.sync(&mut state);
        assert_eq!(ui.panel, Panel::None);
        assert_eq!(ui.open_label(&state), "Resume");
        assert!(!ui.cancels_resume(Command::Open, &state));
        for command in [
            Command::PowerOff,
            Command::Restart,
            Command::Rescan,
            Command::Search,
            Command::Help,
            Command::Details,
            Command::Kernels,
            Command::Snapshots,
            Command::Pools,
            Command::Back,
        ] {
            assert!(!ui.cancels_resume(command, &state), "{command:?}");
        }
        ui.input(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
        assert!(ui.cancels_resume(Command::Open, &state));
        assert_eq!(ui.panel, Panel::None);
        ui.confirm_resume(Command::Open, &state);
        assert_eq!(ui.panel, Panel::ConfirmResume);
        ui.input(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut state);
        assert_eq!(state.resume.session_id(), Some("session-one"));
        assert_eq!(state.selected_target, 1);
        assert!(ui.resume_confirmation.is_none());
    }
    #[test]
    fn explicit_session_actions_confirm_and_standalone_discard_has_no_continuation() {
        let mut state = resume_state();
        let mut ui = Ui::default();
        ui.sync(&mut state);
        for command in [Command::CancelResume, Command::ColdBoot] {
            assert!(ui.reason(command, &state, &options()).is_none());
            assert!(ui.cancels_resume(command, &state));
            ui.confirm_resume(command, &state);
            assert_eq!(ui.panel, Panel::ConfirmResume);
            assert!(matches!(
                ui.input(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut state),
                Input::Command(actual, true) if actual == command
            ));
        }
        assert!(
            super::super::resume_continuation(Command::CancelResume, &ui, &state, &options())
                .unwrap()
                .is_none()
        );
        assert_eq!(state.resume.session_id(), Some("session-one"));
    }
    #[test]
    fn unlock_progress_replaces_stale_locked_labels_without_changing_key_state() {
        let mut state = resume_state();
        state.targets.clear();
        state.environments = vec![serde_json::from_value(serde_json::json!({
            "pool_guid":42,"dataset":"tank/root","is_default":true,
            "targets":[],"default_target":null,"default_error":null,
            "rejected_generations":[],"unavailable":"Locked; unlock to discover boot targets","root":null,
            "properties":{"mountpoint":"/","canmount":"noauto","encryption":"aes-256-gcm",
                "encryptionroot":"tank/root","keystatus":"unavailable","keyformat":"passphrase","active":"on",
                "commandline":null,"kernel":null,"rootprefix":null}
        })).unwrap()];
        let mut ui = Ui {
            unlock_progress: Some(UnlockProgress {
                pool_guid: 42,
                dataset: "tank/root".into(),
                discovering: false,
            }),
            ..Ui::default()
        };
        ui.sync(&mut state);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        for discovering in [false, true] {
            ui.unlock_progress.as_mut().unwrap().discovering = discovering;
            terminal.draw(|f| ui.draw(f, &state, &options())).unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();
            assert!(!text.contains("Locked; unlock"));
            assert!(!text.contains("LOCKED"));
            assert!(text.contains(if discovering {
                "Keys unlocked; finding boot targets"
            } else {
                "Unlocking encryption keys"
            }));
            assert!(state.environments[0].properties.locked());
            assert!(ui.boot_target(&state).is_none());
        }
    }

    #[test]
    fn preferred_variant_never_overrides_the_saved_session() {
        let mut state = resume_state();
        let saved = state.targets[0].clone();
        let mut variant = saved.clone();
        if let zbm_core::boot::Backend::Nixos(n) = &mut variant.backend {
            n.specialisation = Some("niri".into());
            n.toplevel = "/nix/store/niri".into();
        }
        let environment = serde_json::from_value(serde_json::json!({
            "pool_guid":42,"dataset":"tank/root","is_default":true,
            "targets":[saved,variant],"default_target":0,"default_error":null,
            "rejected_generations":[],"unavailable":null,"root":"/guest",
            "properties":{"mountpoint":"/","canmount":"noauto","encryption":"off",
                "encryptionroot":null,"keystatus":"-","keyformat":"none","active":"on",
                "commandline":null,"kernel":null,"rootprefix":null}
        }))
        .unwrap();
        let mut config = options();
        config.config.nixos.preferred_specialisation = Some("niri".into());
        super::super::navigation::merge(&mut state, 42, vec![environment], &config);
        assert_eq!(state.environments[0].default_target, Some(1));
        state.targets.clear();
        let mut ui = Ui::default();
        ui.sync(&mut state);
        assert_eq!(ui.boot_target(&state).unwrap().specialisation(), None);
        let action = super::super::navigation::intent_action(
            super::super::navigation::Intent::Boot,
            &mut state,
            &mut ui,
            &config,
            &mut None,
        )
        .unwrap();
        let super::super::Action::Boot { target, .. } = action else {
            panic!("expected boot")
        };
        assert_eq!(target.specialisation(), None);
        super::super::navigation::intent_action(
            super::super::navigation::Intent::Kernels,
            &mut state,
            &mut ui,
            &config,
            &mut None,
        );
        assert_eq!(state.selected_target, 0);
        state.resume = zbm_core::resume::Status::Inactive;
        state.targets.clear();
        ui.sync(&mut state);
        assert_eq!(
            ui.boot_target(&state).unwrap().specialisation(),
            Some("niri")
        );
    }

    #[test]
    fn cold_boot_pins_the_saved_generation_before_the_environment_default_changes() {
        let mut state = resume_state();
        let saved = state.targets[0].clone();
        let current = state.targets[1].clone();
        state.environments = vec![
            serde_json::from_value(serde_json::json!({
                "pool_guid":42,"dataset":"tank/root","is_default":true,
                "targets":[saved,current],"default_target":1,"default_error":null,
                "rejected_generations":[],"unavailable":null,"root":"/guest",
                "properties":{"mountpoint":"/","canmount":"noauto","encryption":"off",
                    "encryptionroot":null,"keystatus":"-","keyformat":"none","active":"on",
                    "commandline":null,"kernel":null,"rootprefix":null}
            }))
            .unwrap(),
        ];
        state.targets.clear();
        let mut ui = Ui::default();
        ui.sync(&mut state);
        let next = super::super::resume_continuation(Command::ColdBoot, &ui, &state, &options())
            .unwrap()
            .unwrap();
        state.resume = zbm_core::resume::Status::Inactive;
        assert_eq!(ui.boot_target(&state).unwrap().generation(), Some(6));
        let super::super::ResumeContinuation::Action(next) = next else {
            panic!("Cold boot must keep a concrete target");
        };
        let super::super::Action::ColdBoot { target, .. } = *next else {
            panic!("Expected explicit cold-boot operation");
        };
        assert_eq!(target.generation(), Some(7));
    }
    #[test]
    fn session_actions_are_hidden_without_an_image_and_fail_closed_when_unknown() {
        let mut state = resume_state();
        let mut ui = Ui::default();
        ui.sync(&mut state);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 25)).unwrap();
        for present in [true, false] {
            if !present {
                state.resume = zbm_core::resume::Status::Inactive;
            }
            ui.open_panel(Panel::Actions);
            terminal.draw(|f| ui.draw(f, &state, &options())).unwrap();
            for command in [Command::CancelResume, Command::ColdBoot] {
                assert_eq!(ui.menu_commands.contains(&command), present);
                assert_eq!(ui.reason(command, &state, &options()).is_none(), present);
            }
        }
        for status in [
            zbm_core::resume::Status::Locked {
                pool_guid: 42,
                encryption_root: "tank/root".into(),
            },
            zbm_core::resume::Status::Blocked {
                reason: "cannot inspect swap".into(),
            },
        ] {
            state.resume = status;
            for command in [Command::CancelResume, Command::ColdBoot] {
                assert!(ui.reason(command, &state, &options()).is_some());
            }
        }
        state.resume = zbm_core::resume::Status::Unavailable {
            session_id: "valid-header".into(),
            reason: "missing boot record".into(),
        };
        state.pool_view = true;
        ui.sync(&mut state);
        assert!(
            ui.reason(Command::CancelResume, &state, &options())
                .is_none()
        );
        assert!(ui.reason(Command::ColdBoot, &state, &options()).is_some());
    }
    #[test]
    fn explicit_discard_dialog_explains_that_it_stays_in_the_menu_at_compact_sizes() {
        for (width, height) in [(80, 25), (60, 20)] {
            let mut state = resume_state();
            let mut ui = Ui::default();
            ui.sync(&mut state);
            ui.confirm_resume(Command::CancelResume, &state);
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| ui.draw(f, &state, &options())).unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();
            assert!(
                rendered.contains("No system will boot"),
                "{width}x{height}: {rendered}"
            );
            ui.input(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut state);
            assert_eq!(state.resume.session_id(), Some("session-one"));
        }
    }
    #[test]
    fn f10_never_confirms_cancellation_and_rollback_keeps_typed_confirmation() {
        let mut state = resume_state();
        let mut ui = Ui::default();
        ui.sync(&mut state);
        for panel in [
            Panel::ConfirmResume,
            Panel::ConfirmRollback,
            Panel::ConfirmPromote,
            Panel::ConfirmDiscard,
            Panel::Unlock,
            Panel::Help,
            Panel::Details,
            Panel::Actions,
        ] {
            ui.open_panel(panel);
            ui.resume_confirmation = Some(("session-one".into(), Command::Open));
            assert!(matches!(
                ui.input(
                    KeyEvent::new(KeyCode::F(10), KeyModifiers::NONE),
                    &mut state
                ),
                Input::Command(Command::PowerOff, false)
            ));
            assert_eq!(state.resume.session_id(), Some("session-one"));
        }
        ui.confirm_resume(Command::Rollback, &state);
        assert_eq!(ui.panel, Panel::ConfirmRollback);
        assert!(!matches!(
            ui.input(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &mut state
            ),
            Input::Command(_, true)
        ));
        for c in "ROLLBACK".chars() {
            ui.input(key(c), &mut state);
        }
        assert!(matches!(
            ui.input(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &mut state
            ),
            Input::Command(Command::Rollback, true)
        ));
    }
    #[test]
    fn compact_resume_indicator_and_combined_dialog_fit() {
        let mut state = resume_state();
        let mut ui = Ui::default();
        ui.sync(&mut state);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 20)).unwrap();
        terminal.draw(|f| ui.draw(f, &state, &options())).unwrap();
        let text = |t: &ratatui::Terminal<ratatui::backend::TestBackend>| {
            t.backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };
        assert!(text(&terminal).contains("[Resume pending]"));
        assert!(text(&terminal).contains("Resume"));
        ui.confirm_resume(Command::Rollback, &state);
        terminal.draw(|f| ui.draw(f, &state, &options())).unwrap();
        let rendered = text(&terminal);
        assert!(rendered.contains("discards the saved session"));
        assert!(rendered.contains("Type ROLLBACK"), "{rendered}");
        assert!(rendered.contains("Power off"));
    }
    fn options() -> Options {
        Options {
            config: crate::config::Config::default(),
            preview: false,
            supervised: true,
            events: crate::diagnostics::Events::default(),
        }
    }
    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    #[test]
    fn variants_of_one_generation_have_distinct_selectable_rows() {
        let base: BootTarget = serde_json::from_value(serde_json::json!({
            "dataset": "tank/root", "generation": 7, "label": "base", "root": "/guest",
            "toplevel": "/nix/store/base", "backend": {"kind": "Nixos"}, "snapshot": null,
            "mountpoint": null, "issue": null,
            "inputs": {"kernel": "/shared/kernel", "initrd": null, "init": "/nix/store/base/init", "kernel_params": []}
        })).unwrap();
        let mut child = serde_json::to_value(&base).unwrap();
        child["specialisation"] = "niri".into();
        child["toplevel"] = "/nix/store/child".into();
        let child: BootTarget = serde_json::from_value(child).unwrap();
        assert_ne!(target_id(&base), target_id(&child));
        assert_eq!(
            variant_title(7, child.specialisation()),
            "Generation 7 · niri"
        );
        let state = State {
            targets: vec![base, child],
            ..State::default()
        };
        let rows = Ui::default().rows(&state);
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].id, rows[1].id);
    }

    #[test]
    fn filter_never_runs_letter_commands_and_function_recovery_stays_available() {
        let mut state = State {
            pools: zbm_core::preview(),
            ..State::default()
        };
        let mut ui = Ui::default();
        ui.sync(&mut state);
        ui.searching = true;
        for c in ['p', 's', 'n', 'q'] {
            assert!(matches!(
                ui.input(key(c), &mut state),
                Input::Changed("filter")
            ));
        }
        assert!(matches!(
            ui.input(KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE), &mut state),
            Input::Command(Command::Shell, false)
        ));
        assert!(ui.reason(Command::Open, &state, &options()).is_some());
    }

    #[test]
    fn discard_confirmation_does_not_accept_modified_enter() {
        let mut state = State::default();
        let mut ui = Ui::default();
        ui.sync(&mut state);
        ui.open_panel(Panel::ConfirmDiscard);
        assert!(matches!(
            ui.input(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT), &mut state),
            Input::Ignored
        ));
        assert_eq!(ui.panel, Panel::ConfirmDiscard);
        assert!(matches!(
            ui.input(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &mut state
            ),
            Input::Command(Command::Discard, true)
        ));
    }
    #[test]
    fn rollback_needs_exact_text_and_ignores_letter_shortcuts() {
        let mut state = State {
            pools: zbm_core::preview(),
            ..State::default()
        };
        let mut ui = Ui::default();
        ui.sync(&mut state);
        ui.open_panel(Panel::ConfirmRollback);
        assert!(matches!(
            ui.input(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &mut state
            ),
            Input::Ignored
        ));
        for c in "ROLLBACK".chars() {
            assert!(matches!(ui.input(key(c), &mut state), Input::Changed(_)));
        }
        assert!(matches!(
            ui.input(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT), &mut state),
            Input::Ignored
        ));
        assert!(matches!(
            ui.input(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &mut state
            ),
            Input::Command(Command::Rollback, true)
        ));
        ui.open_panel(Panel::ConfirmRollback);
        ui.input(key('p'), &mut state);
        assert_eq!(ui.panel, Panel::ConfirmRollback);
        assert_eq!(ui.confirmation, "p");
        ui.input(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut state);
        assert_eq!(ui.panel, Panel::None);
    }
    #[test]
    fn every_snapshot_mutation_is_disabled_in_preview() {
        let mut state = State {
            snapshot_view: true,
            snapshots: vec![zbm_core::boot_zfs::Snapshot {
                source: zbm_core::boot::SnapshotSource {
                    name: "tank/root@good".into(),
                    guid: 1,
                },
                creation: 1,
            }],
            ..State::default()
        };
        let mut ui = Ui::default();
        ui.sync(&mut state);
        let mut options = options();
        options.preview = true;
        for command in [
            Command::Clone,
            Command::ClonePromote,
            Command::Rollback,
            Command::Discard,
            Command::Prepare,
        ] {
            assert!(
                ui.reason(command, &state, &options)
                    .unwrap()
                    .contains("preview")
            );
        }
    }

    #[test]
    fn closing_help_resumes_search_so_typing_cannot_power_off() {
        let mut state = State {
            pools: zbm_core::preview(),
            ..State::default()
        };
        let mut ui = Ui::default();
        ui.sync(&mut state);
        ui.searching = true;
        ui.input(key('z'), &mut state);
        ui.open_panel(Panel::Help);
        ui.input(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut state);
        assert!(ui.searching);
        assert!(matches!(
            ui.input(key('p'), &mut state),
            Input::Changed("filter")
        ));
        assert_eq!(ui.screens[&ui.key].query, "zp");
    }
    #[test]
    fn layouts_and_full_action_inventory_fit_wide_and_small_consoles() {
        for (width, height) in [(160, 50), (80, 25), (60, 20)] {
            let mut state = State {
                pools: zbm_core::preview(),
                ..State::default()
            };
            let mut ui = Ui::default();
            ui.sync(&mut state);
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| ui.draw(frame, &state, &options()))
                .unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                text.contains("Rescan")
                    && text.contains("Shell")
                    && text.contains("Power off")
                    && text.contains("Details"),
                "{width}x{height}: {text}"
            );
            ui.open_panel(Panel::Actions);
            terminal
                .draw(|frame| ui.draw(frame, &state, &options()))
                .unwrap();
            // The menu may scroll, but every command and its reason must remain reachable.
            for command in ui.menu_commands.clone() {
                ui.menu
                    .select(ui.menu_commands.iter().position(|c| *c == command));
                terminal
                    .draw(|frame| ui.draw(frame, &state, &options()))
                    .unwrap();
                let menu = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|c| c.symbol())
                    .collect::<String>();
                assert!(menu.contains("Esc Close"), "{width}x{height}: {menu}");
                if ui.reason(command, &state, &options()).is_some() {
                    assert!(menu.contains("Unavailable:"), "{width}x{height}: {menu}");
                } else {
                    assert!(menu.contains("Enter:"), "{width}x{height}: {menu}");
                }
                assert!(
                    matches!(ui.input(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut state),
                    Input::Command(actual, false) if actual == command)
                );
                ui.open_panel(Panel::Actions);
                terminal
                    .draw(|frame| ui.draw(frame, &state, &options()))
                    .unwrap();
            }
        }
    }

    #[test]
    fn reading_panels_capture_commands_but_keep_function_recovery() {
        let mut state = State {
            pools: zbm_core::preview(),
            ..State::default()
        };
        let mut ui = Ui::default();
        ui.sync(&mut state);
        for panel in [Panel::Help, Panel::Details] {
            ui.open_panel(panel);
            for code in [
                KeyCode::Enter,
                KeyCode::Char('b'),
                KeyCode::Char('p'),
                KeyCode::Char('q'),
                KeyCode::Char('c'),
                KeyCode::Char('d'),
                KeyCode::Char('r'),
                KeyCode::Char('u'),
                KeyCode::Char('/'),
                KeyCode::F(2),
                KeyCode::F(5),
            ] {
                assert!(
                    matches!(
                        ui.input(KeyEvent::new(code, KeyModifiers::NONE), &mut state),
                        Input::Ignored
                    ),
                    "{panel:?} {code:?}"
                );
                assert_eq!(ui.panel, panel);
            }
            for (code, expected) in [
                (KeyCode::F(4), Command::Shell),
                (KeyCode::F(6), Command::Restart),
                (KeyCode::F(10), Command::PowerOff),
            ] {
                assert!(
                    matches!(ui.input(KeyEvent::new(code, KeyModifiers::NONE), &mut state),
                    Input::Command(actual, false) if actual == expected)
                );
            }
            ui.input(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut state);
            assert_eq!(ui.panel, Panel::None);
        }
    }

    #[test]
    fn compact_list_shows_more_entries_and_pages_by_viewport() {
        let pool = zbm_core::preview().remove(0);
        let mut state = State {
            pools: (0..20)
                .map(|i| {
                    let mut pool = pool.clone();
                    pool.guid = i;
                    pool.name = format!("pool-{i:02}");
                    pool
                })
                .collect(),
            ..State::default()
        };
        let mut ui = Ui::default();
        ui.sync(&mut state);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 25)).unwrap();
        terminal.draw(|f| ui.draw(f, &state, &options())).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains("pool-05"), "{text}");
        ui.input(
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(state.selected, ui.page_size);
        ui.open_panel(Panel::Help);
        terminal.draw(|f| ui.draw(f, &state, &options())).unwrap();
        ui.input(KeyEvent::new(KeyCode::End, KeyModifiers::NONE), &mut state);
        let end = ui.scroll;
        assert!(end > 0);
        for _ in 0..100 {
            ui.input(
                KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
                &mut state,
            );
        }
        assert_eq!(ui.scroll, end);
        terminal.draw(|f| ui.draw(f, &state, &options())).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains("not the machine"));
    }

    #[test]
    fn unavailable_generation_is_inspectable_but_never_becomes_a_boot_target() {
        use zbm_core::boot::GenerationIssue;
        let target = BootTarget {
            dataset: "tank/root".into(),
            label: "NixOS".into(),
            root: "/run/fixture".into(),
            snapshot: None,
            mountpoint: None,
            backend: zbm_core::boot::Backend::Nixos(zbm_core::boot::NixosTarget {
                generation: 1,
                specialisation: None,
                toplevel: "/nix/store/fixture".into(),
                kernel: "/nix/store/fixture/kernel".into(),
                initrd: None,
                init: "/nix/store/fixture/init".into(),
                kernel_params: vec![],
            }),
        };
        let mut state = State {
            targets: vec![target],
            rejected_generations: vec![GenerationIssue {
                generation: 2,
                specialisation: None,
                error: "Invalid Bootspec".into(),
            }],
            ..State::default()
        };
        let mut ui = Ui::default();
        ui.sync(&mut state);
        assert!(ui.target(&state).is_some());
        ui.input(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &mut state);
        assert!(ui.target(&state).is_none());
        assert!(
            ui.reason(Command::Open, &state, &options())
                .unwrap()
                .contains("Invalid Bootspec")
        );
        assert_eq!(state.selected_target, 0);
    }

    #[test]
    fn default_and_encrypted_environment_diagnostics_remain_visible() {
        use zbm_core::environment::{BootEnvironment, BootProperties};
        let mut state = State {
            environments: vec![BootEnvironment {
                pool_guid: 42,
                default_target: None,
                default_error: None,
                dataset: "tank/ROOT/encrypted".into(),
                root: None,
                is_default: true,
                targets: vec![],
                rejected_generations: vec![],
                unavailable: Some("Locked; unlock to discover boot targets".into()),
                properties: BootProperties {
                    mountpoint: "/".into(),
                    canmount: "noauto".into(),
                    encryption: "aes-256-gcm".into(),
                    encryptionroot: Some("tank".into()),
                    keystatus: "unavailable".into(),
                    keyformat: "passphrase".into(),
                    keylocation: None,
                    keysource: None,
                    active: "on".into(),
                    commandline: None,
                    kernel: None,
                    rootprefix: None,
                },
            }],
            ..State::default()
        };
        let mut ui = Ui::default();
        ui.sync(&mut state);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 50)).unwrap();
        terminal
            .draw(|frame| ui.draw(frame, &state, &options()))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(
            text.contains("* tank/ROOT/encrypted")
                && text.contains("BOOTFS")
                && text.contains("Locked")
        );
        assert!(ui.reason(Command::Open, &state, &options()).is_none());
        ui.open_panel(Panel::Unlock);
        for c in "secret-snpq".chars() {
            assert!(matches!(ui.input(key(c), &mut state), Input::Ignored));
        }
        assert_eq!(ui.panel, Panel::Unlock);
        terminal
            .draw(|frame| ui.draw(frame, &state, &options()))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(!text.contains("secret-snpq"));
        assert!(
            !serde_json::to_string(&ui.telemetry(&state))
                .unwrap()
                .contains("secret-snpq")
        );
        assert!(matches!(
            ui.input(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT), &mut state),
            Input::Ignored
        ));
        let Input::Unlock(secret) = ui.input(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
        ) else {
            panic!("Expected unlock input");
        };
        assert_eq!(secret.as_slice(), b"secret-snpq");
        assert!(ui.passphrase.is_empty());
        ui.open_panel(Panel::Unlock);
        ui.input(key('x'), &mut state);
        ui.input(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut state);
        assert!(ui.passphrase.is_empty());
        ui.open_panel(Panel::Unlock);
        ui.input(key('x'), &mut state);
        assert!(matches!(
            ui.input(KeyEvent::new(KeyCode::F(6), KeyModifiers::NONE), &mut state),
            Input::Command(Command::Restart, false)
        ));
        assert!(ui.passphrase.is_empty());
    }
}
