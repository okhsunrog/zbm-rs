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
        if !state.targets.is_empty() || !state.rejected_generations.is_empty() {
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
    Kernel(String),
    Rejected(u64),
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
    Unlock,
}

#[derive(Default)]
pub struct Ui {
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
    #[cfg(feature = "gui-prototype")]
    revision: i32,
    #[cfg(feature = "gui-prototype")]
    gui_context: String,
    passphrase: zeroize::Zeroizing<String>,
}

pub enum Input {
    Unlock(zeroize::Zeroizing<Vec<u8>>),
    Changed(&'static str),
    Command(Command, bool),
    Ignored,
}

fn target_id(t: &BootTarget) -> RowId {
    t.generation()
        .map(RowId::Generation)
        .unwrap_or_else(|| RowId::Kernel(t.inputs().kernel.display().to_string()))
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
    #[cfg(feature = "gui-prototype")]
    pub(crate) fn gui_snapshot(
        &mut self,
        state: &State,
        options: &Options,
    ) -> crate::gui::Snapshot {
        use crate::gui::{AvailableAction, Row, Snapshot};
        let rows = self.visible(state);
        let selected = self.selected(state).map(|row| row.id);
        // A repaint must not invalidate input; changing targets, selection or a
        // confirmation does. Never carry secrets in this comparison key.
        let context = format!(
            "{}:{:?}:{:?}:{:?}:{}",
            self.key,
            rows.iter().map(|r| &r.id).collect::<Vec<_>>(),
            selected,
            self.panel,
            state.activity.is_idle()
        );
        if context != self.gui_context {
            self.gui_context = context;
            self.revision = self.revision.wrapping_add(1);
        }
        Snapshot {
            query: self
                .screens
                .get(&self.key)
                .map(|s| s.query.clone())
                .unwrap_or_default(),
            revision: self.revision,
            heading: View::of(state).title().into(),
            selected: rows
                .iter()
                .position(|row| Some(&row.id) == selected.as_ref())
                .map_or(-1, |i| i as i32),
            rows: rows
                .into_iter()
                .map(|row| Row {
                    title: row.title,
                    subtitle: row.subtitle,
                    unavailable: row.tag.contains("UNAVAILABLE"),
                    badge: if row.tag.contains("UNAVAILABLE") {
                        "UNAVAILABLE"
                    } else {
                        row.tag
                    }
                    .into(),
                })
                .collect(),
            details: self
                .details(state, options)
                .iter()
                .map(|line| {
                    line.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n"),
            status: state
                .error
                .clone()
                .unwrap_or_else(|| match &state.activity {
                    zbm_core::activity::Activity::Scanning => "Discovering ZFS pools…".into(),
                    zbm_core::activity::Activity::Running(description) => description.clone(),
                    zbm_core::activity::Activity::RestartRequired => {
                        "Restart required to reconcile interrupted work".into()
                    }
                    _ => String::new(),
                }),
            primary: self.open_label(state).into(),
            primary_enabled: self.panel == Panel::None
                && self.reason(Command::Open, state, options).is_none(),
            actions: ALL
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    !matches!(
                        c,
                        Command::Open
                            | Command::Search
                            | Command::Details
                            | Command::Help
                            | Command::Actions
                            | Command::Exit
                    )
                })
                .map(|(id, c)| {
                    let reason = self.reason(*c, state, options);
                    AvailableAction {
                        id: id as i32,
                        label: c.label().into(),
                        enabled: reason.is_none() && (self.panel == Panel::None || c.recovery()),
                        reason: reason.unwrap_or_default(),
                    }
                })
                .collect(),
            panel: self.panel,
            confirmation: match self.panel {
                Panel::Unlock => "Unlock selected boot environment".into(),
                Panel::ConfirmRollback => {
                    "Type ROLLBACK to permanently roll back the selected dataset".into()
                }
                Panel::ConfirmDiscard => "Discard the owned prepared clone?".into(),
                Panel::ConfirmPromote => "Clone and promote the selected snapshot?".into(),
                _ => String::new(),
            },
            security: if options.config.security.mode == crate::config::SecurityMode::Enforce {
                "Protected mode · authorization checked before boot"
            } else {
                "Signature enforcement is off"
            }
            .into(),
            editable: state.activity.is_idle() && self.panel == Panel::None,
        }
    }

    #[cfg(feature = "gui-prototype")]
    pub(crate) fn gui_input(&mut self, event: crate::gui::Event, state: &mut State) -> Input {
        use crate::gui::Event;
        if event.revision() != self.revision {
            return Input::Ignored;
        }
        let editable = state.activity.is_idle() && self.panel == Panel::None;
        match event {
            Event::Select { row, .. } if editable => {
                if let Some(row) = usize::try_from(row)
                    .ok()
                    .and_then(|i| self.visible(state).into_iter().nth(i))
                {
                    self.screens.get_mut(&self.key).unwrap().selected = Some(row.id);
                    state.error = None;
                    self.sync(state);
                    return Input::Changed("selection");
                }
            }
            Event::Move { delta, .. } if editable => {
                let rows = self.visible(state);
                if !rows.is_empty() {
                    let screen = self.screens.get_mut(&self.key).unwrap();
                    let index = (screen.list.selected().unwrap_or(0) as i64 + i64::from(delta))
                        .clamp(0, rows.len() as i64 - 1) as usize;
                    screen.selected = Some(rows[index].id.clone());
                    state.error = None;
                    self.sync(state);
                    return Input::Changed("selection");
                }
            }
            Event::Filter { text, .. } if editable => {
                self.screens.get_mut(&self.key).unwrap().query = text.chars().take(256).collect();
                self.sync(state);
                return Input::Changed("filter");
            }
            Event::Command { id, .. } => {
                if let Some(command) = usize::try_from(id).ok().and_then(|i| ALL.get(i)).copied()
                    && (self.panel == Panel::None || command.recovery())
                {
                    return Input::Command(command, false);
                }
            }
            Event::Unlock { passphrase, .. } if self.panel == Panel::Unlock => {
                if !passphrase.is_empty() && passphrase.len() <= 1024 {
                    self.panel = Panel::None;
                    return Input::Unlock(passphrase);
                }
            }
            Event::Confirm { text, .. }
                if matches!(
                    self.panel,
                    Panel::ConfirmRollback | Panel::ConfirmDiscard | Panel::ConfirmPromote
                ) =>
            {
                self.confirmation = text;
                return self.input(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), state);
            }
            Event::Cancel { .. } => {
                return self.input(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), state);
            }
            _ => {}
        }
        Input::Ignored
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
    fn rows(state: &State) -> Vec<Row> {
        match View::of(state) {
            View::Pools => state
                .pools
                .iter()
                .enumerate()
                .map(|(i, p)| Row {
                    id: RowId::Pool(p.guid),
                    kind: Kind::Pool(i),
                    title: p.name.clone(),
                    subtitle: p.health.clone(),
                    tag: "POOL",
                })
                .collect(),
            View::Environments => state
                .environments
                .iter()
                .enumerate()
                .map(|(i, e)| Row {
                    id: RowId::Environment(e.dataset.clone()),
                    kind: Kind::Environment(i),
                    title: format!("{}{}", if e.is_default { "* " } else { "" }, e.dataset),
                    subtitle: e.unavailable.clone().unwrap_or_else(|| {
                        format!(
                            "{} targets / {} unavailable",
                            e.targets.len(),
                            e.rejected_generations.len()
                        )
                    }),
                    tag: if e.is_default && e.unavailable.is_some() {
                        "BOOTFS / UNAVAILABLE"
                    } else if e.unavailable.is_some() {
                        "UNAVAILABLE"
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
                                    .map(|g| format!("Generation {g}"))
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
                                id: RowId::Rejected(e.generation),
                                kind: Kind::Rejected(i),
                                title: format!("Generation {}", e.generation),
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
        Self::rows(state)
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
            View::Environments => format!("{pool}/environments"),
            View::Snapshots => format!("{pool}/{environment}/snapshots"),
            View::Generations | View::Kernels => {
                format!("{pool}/{environment}/{snapshot}/generations")
            }
        };
        if self.key != key {
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
                .map(|e| RowId::Environment(e.dataset.clone())),
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
                Kind::Environment(i) => state.selected_environment = i,
                Kind::Snapshot(i) => state.selected_snapshot = i,
                Kind::Generation(i) => state.selected_target = i,
                Kind::Rejected(_) => {}
            }
        }
    }
    pub fn telemetry(&self, state: &State) -> serde_json::Value {
        serde_json::json!({"view":View::of(state),"filter":self.screens.get(&self.key).map(|s|s.query.as_str()).unwrap_or(""),"searching":self.searching,"panel":self.panel,"selected_row":self.selected(state).map(|r|r.id)})
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
            Command::Help | Command::Actions | Command::Details | Command::Exit | Command::Shell
        ) {
            return None;
        }
        if matches!(command, Command::Restart | Command::PowerOff) {
            return (!options.supervised).then(|| "Requires the PID 1 supervisor".into());
        }
        if state.activity.requires_restart() {
            return Some("Restart manager to reconcile the timed-out operation".into());
        }
        if state.activity.operation().is_some() || state.activity.is_scanning() {
            return Some(
                "An operation is in progress; Restart and Power off remain available".into(),
            );
        }
        if matches!(command, Command::Search | Command::Back | Command::Rescan) {
            return None;
        }
        if !options.supervised || options.preview {
            return Some("Boot actions are unavailable in local preview".into());
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
            Some(Kind::Environment(i)) if state.environments[i].properties.can_unlock() => None,
            Some(Kind::Environment(i)) if state.environments[i].root.is_none() => state
                .environments[i]
                .unavailable
                .clone()
                .or(Some("Boot environment is unavailable".into())),
            _ => None,
        }
    }
    pub fn open_label(&self, state: &State) -> &'static str {
        match View::of(state) {
            View::Pools => "Import",
            View::Environments
                if state
                    .environments
                    .get(state.selected_environment)
                    .is_some_and(|e| e.properties.can_unlock()) =>
            {
                "Unlock"
            }
            View::Environments => "Open",
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
                self.panel = Panel::None;
                self.searching = std::mem::take(&mut self.resume_search);
                return Input::Changed("panel-closed");
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
    fn options() -> Options {
        Options {
            config: crate::config::Config::default(),
            preview: false,
            supervised: true,
            events: None,
        }
    }
    #[cfg(feature = "gui-prototype")]
    #[test]
    fn gui_rejects_stale_targets_but_accepts_input_after_repaint() {
        use crate::gui::Event;
        let mut state = State {
            pools: zbm_core::preview(),
            ..State::default()
        };
        state.pools.push(zbm_core::Pool {
            name: "other".into(),
            guid: 43,
            health: "ONLINE".into(),
        });
        let mut ui = Ui::default();
        ui.sync(&mut state);
        let revision = ui.gui_snapshot(&state, &options()).revision;
        assert_eq!(ui.gui_snapshot(&state, &options()).revision, revision);
        assert!(matches!(
            ui.gui_input(Event::Select { revision, row: 1 }, &mut state),
            Input::Changed("selection")
        ));
        let next = ui.gui_snapshot(&state, &options()).revision;
        assert_ne!(revision, next);
        assert!(matches!(
            ui.gui_input(Event::Command { revision, id: 0 }, &mut state),
            Input::Ignored
        ));
        assert!(matches!(
            ui.gui_input(
                Event::Select {
                    revision: next,
                    row: -1
                },
                &mut state
            ),
            Input::Ignored
        ));
        assert!(matches!(
            ui.gui_input(
                Event::Command {
                    revision: next,
                    id: i32::MAX
                },
                &mut state
            ),
            Input::Ignored
        ));
        state.activity = zbm_core::activity::Activity::Scanning;
        let busy = ui.gui_snapshot(&state, &options());
        assert!(!busy.primary_enabled && !busy.editable);
        assert!(matches!(
            ui.gui_input(
                Event::Select {
                    revision: busy.revision,
                    row: 0
                },
                &mut state
            ),
            Input::Ignored
        ));
    }

    #[cfg(feature = "gui-prototype")]
    #[test]
    fn gui_modal_confirmation_and_security_use_shared_policy() {
        use crate::gui::Event;
        let mut state = State::default();
        let mut ui = Ui::default();
        ui.sync(&mut state);
        let mut options = options();
        options.config.security.mode = crate::config::SecurityMode::Enforce;
        let snapshot = ui.gui_snapshot(&state, &options);
        for command in [
            Command::Shell,
            Command::Rollback,
            Command::Clone,
            Command::ClonePromote,
        ] {
            let id = ALL.iter().position(|c| *c == command).unwrap() as i32;
            let action = snapshot.actions.iter().find(|a| a.id == id).unwrap();
            assert!(!action.enabled && !action.reason.is_empty());
        }
        ui.open_panel(Panel::ConfirmRollback);
        let revision = ui.gui_snapshot(&state, &options).revision;
        assert!(matches!(
            ui.gui_input(Event::Command { revision, id: 0 }, &mut state),
            Input::Ignored
        ));
        assert!(matches!(
            ui.gui_input(
                Event::Confirm {
                    revision,
                    text: "rollback".into()
                },
                &mut state
            ),
            Input::Ignored
        ));
        assert_eq!(ui.panel, Panel::ConfirmRollback);
        assert!(matches!(
            ui.gui_input(
                Event::Confirm {
                    revision,
                    text: "ROLLBACK".into()
                },
                &mut state
            ),
            Input::Command(Command::Rollback, true)
        ));
        ui.open_panel(Panel::Unlock);
        let revision = ui.gui_snapshot(&state, &options).revision;
        assert!(matches!(
            ui.gui_input(
                Event::Unlock {
                    revision,
                    passphrase: zeroize::Zeroizing::new(vec![])
                },
                &mut state
            ),
            Input::Ignored
        ));
        assert!(matches!(
            ui.gui_input(
                Event::Unlock {
                    revision,
                    passphrase: zeroize::Zeroizing::new(vec![1; 1025])
                },
                &mut state
            ),
            Input::Ignored
        ));
        assert!(matches!(
            ui.gui_input(
                Event::Unlock {
                    revision,
                    passphrase: zeroize::Zeroizing::new(b"fixture-only".to_vec())
                },
                &mut state
            ),
            Input::Unlock(_)
        ));
        assert_eq!(ui.panel, Panel::None);
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
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
            for command in ALL.into_iter().filter(|c| *c != Command::Actions) {
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
        #[cfg(feature = "gui-prototype")]
        {
            let snapshot = ui.gui_snapshot(&state, &options());
            assert!(!snapshot.primary_enabled);
            assert!(snapshot.rows[snapshot.selected as usize].unavailable);
            assert!(snapshot.details.contains("Invalid Bootspec"));
        }
    }

    #[test]
    fn default_and_encrypted_environment_diagnostics_remain_visible() {
        use zbm_core::environment::{BootEnvironment, BootProperties};
        let mut state = State {
            environments: vec![BootEnvironment {
                dataset: "tank/ROOT/encrypted".into(),
                root: None,
                is_default: true,
                targets: vec![],
                rejected_generations: vec![],
                unavailable: Some(
                    "Encryption key is unavailable; press Enter to unlock with a passphrase".into(),
                ),
                properties: BootProperties {
                    mountpoint: "/".into(),
                    canmount: "noauto".into(),
                    encryption: "aes-256-gcm".into(),
                    encryptionroot: Some("tank".into()),
                    keystatus: "unavailable".into(),
                    keyformat: "passphrase".into(),
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
                && text.contains("Encryption key is unavailable")
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
