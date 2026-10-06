//! Presentation and keyboard navigation. No filesystem reads or ZFS effects.
use super::{
    Options,
    command::{ALL, Command},
    status_message,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
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
                .is_some_and(|t| t.generation.is_none())
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
}

#[derive(Default)]
pub struct Ui {
    screens: BTreeMap<String, Screen>,
    key: String,
    pub searching: bool,
    resume_search: bool,
    pub panel: Panel,
    menu: ListState,
    scroll: u16,
    pub prepared: Option<BootPlan>,
    pub operation_started: Option<Instant>,
    confirmation: String,
}

pub enum Input {
    Changed(&'static str),
    Command(Command, bool),
    Ignored,
}

fn target_id(t: &BootTarget) -> RowId {
    t.generation
        .map(RowId::Generation)
        .unwrap_or_else(|| RowId::Kernel(t.inputs.kernel.display().to_string()))
}

fn clean(value: impl AsRef<str>) -> String {
    value
        .as_ref()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
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
    pub fn reset_after_mutation(&mut self) {
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
                    subtitle: format!("{}  GUID {}", p.health, p.guid),
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
                    subtitle: format!("created {}  GUID {}", s.creation, s.source.guid),
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
                            t.generation.unwrap_or(0),
                            Row {
                                id: target_id(t),
                                kind: Kind::Generation(i),
                                title: t
                                    .generation
                                    .map(|g| format!("Generation {g}"))
                                    .unwrap_or_else(|| t.label.clone()),
                                subtitle: t.issue.clone().unwrap_or_else(|| {
                                    if t.generation.is_some() {
                                        t.label.clone()
                                    } else {
                                        t.inputs
                                            .initrd
                                            .as_ref()
                                            .map(|p| p.display().to_string())
                                            .unwrap_or_default()
                                    }
                                }),
                                tag: if t.issue.is_some() {
                                    "UNAVAILABLE"
                                } else if t.snapshot.is_some() {
                                    "SNAPSHOT"
                                } else {
                                    if t.generation.is_some() {
                                        "BOOTSPEC"
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
        if state.operation.is_none() {
            self.operation_started = None;
        }
        let view = View::of(state);
        let pool = state.pools.get(state.selected).map(|p| p.guid).unwrap_or(0);
        let environment = state
            .environments
            .get(state.selected_environment)
            .map(|e| e.dataset.as_str())
            .unwrap_or("");
        let snapshot = if state.snapshot_view && view == View::Generations {
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
        if matches!(
            command,
            Command::Help | Command::Actions | Command::Details | Command::Exit | Command::Shell
        ) {
            return None;
        }
        if matches!(command, Command::Restart | Command::PowerOff) {
            return (!options.supervised).then(|| "Requires the PID 1 supervisor".into());
        }
        if state.requires_restart {
            return Some(
                "Restart manager or use Shell to reconcile the timed-out operation".into(),
            );
        }
        if state.operation.is_some() || state.scanning {
            return Some(
                "An operation is in progress; Shell, Restart and Power off remain available".into(),
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
            if let Some(issue) = self.target(state).and_then(|t| t.issue.clone()) {
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
        if let Some(issue) = self.target(state).and_then(|t| t.issue.clone()) {
            return Some(issue);
        }
        match self.selected(state).map(|row| row.kind) {
            None => Some("No selectable target matches this filter".into()),
            Some(Kind::Rejected(i)) => Some(format!(
                "generation {}: {}",
                state.rejected_generations[i].generation, state.rejected_generations[i].error
            )),
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
            View::Pools => "Import / inspect pool",
            View::Environments => "Inspect boot targets",
            View::Snapshots => "Inspect snapshot boot targets",
            View::Generations | View::Kernels if state.snapshot_view => {
                "Prepare / reuse clone & boot"
            }
            View::Generations | View::Kernels => "Boot selected target",
        }
    }
    pub fn open_panel(&mut self, panel: Panel) {
        self.confirmation.clear();
        if self.panel == Panel::None && self.searching {
            self.resume_search = true;
        }
        self.searching = false;
        self.panel = panel;
        self.scroll = 0;
        self.menu.select(Some(0));
    }
    pub fn input(&mut self, key: KeyEvent, state: &mut State) -> Input {
        let command = Command::from_key(key);
        if matches!(key.code, KeyCode::F(_))
            && let Some(command) = command
        {
            return Input::Command(command, false);
        }
        if self.searching {
            if state.operation.is_some() || state.scanning || state.requires_restart {
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
        if command.is_some_and(Command::recovery) {
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
                        (self.menu.selected().unwrap_or(0) + ALL.len() - 1) % ALL.len(),
                    )),
                    KeyCode::Down => self
                        .menu
                        .select(Some((self.menu.selected().unwrap_or(0) + 1) % ALL.len())),
                    KeyCode::Enter if command == Some(Command::Open) => {
                        let command = ALL[self.menu.selected().unwrap_or(0)];
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
                    KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
                    KeyCode::PageDown => self.scroll = self.scroll.saturating_add(8),
                    KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
                    KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(8),
                    KeyCode::Home => self.scroll = 0,
                    _ => {
                        if let Some(command) = command {
                            return Input::Command(command, false);
                        } else {
                            return Input::Ignored;
                        }
                    }
                }
            }
            return Input::Changed("panel");
        }
        if key.code == KeyCode::Esc && !self.screens[&self.key].query.is_empty() {
            self.screens.get_mut(&self.key).unwrap().query.clear();
            self.sync(state);
            return Input::Changed("filter");
        }
        if state.operation.is_none() && !state.requires_restart && !state.scanning {
            let rows = self.visible(state);
            let screen = self.screens.get_mut(&self.key).unwrap();
            let current = screen.list.selected().unwrap_or(0);
            let next = match key.code {
                KeyCode::Up => Some((current + rows.len().saturating_sub(1)) % rows.len().max(1)),
                KeyCode::Down => Some((current + 1) % rows.len().max(1)),
                KeyCode::Home => Some(0),
                KeyCode::End => Some(rows.len().saturating_sub(1)),
                KeyCode::PageDown => Some((current + 8).min(rows.len().saturating_sub(1))),
                KeyCode::PageUp => Some(current.saturating_sub(8)),
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
    fn details(&self, state: &State, options: &Options) -> Vec<Line<'static>> {
        let mut lines = vec![];
        let mut field = |key: &str, value: String| {
            lines.push(Line::from(vec![
                Span::styled(format!("{key}: "), Style::default().fg(Color::Gray)),
                Span::raw(clean(value)),
            ]));
        };
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
                if let Some(issue) = &t.issue {
                    field("Unavailable", issue.clone());
                }
                if let Some(g) = t.generation {
                    field("Generation", g.to_string());
                }
                field("Label", t.label.clone());
                field("Dataset", t.dataset.clone());
                if let Some(p) = &t.toplevel {
                    field("Toplevel", p.display().to_string());
                }
                field("Kernel", t.inputs.kernel.display().to_string());
                field(
                    "Initrd",
                    t.inputs
                        .initrd
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or("None".into()),
                );
                if let Some(p) = &t.inputs.init {
                    field("Init", p.display().to_string());
                }
                if let Some(s) = &t.snapshot {
                    field("Snapshot", s.name.clone());
                    field("Snapshot GUID", s.guid.to_string());
                    field("Root override", "Validated during clone preparation".into());
                }
                let plan = self.prepared.as_ref().filter(|p| {
                    p.target.generation == t.generation
                        && p.target.inputs.kernel == t.inputs.kernel
                        && p.target
                            .snapshot
                            .as_ref()
                            .zip(t.snapshot.as_ref())
                            .is_some_and(|(a, b)| a.guid == b.guid && a.name == b.name)
                });
                if let Some(plan) = plan {
                    field("Prepared clone", plan.target.dataset.clone());
                    field("Prepared cmdline", plan.cmdline.join(" "));
                } else {
                    let mut args = t.inputs.command_line(&options.config.kernel_args);
                    if let zbm_core::boot::Backend::Linux { rootprefix, .. } = &t.backend {
                        args.retain(|arg| !arg.starts_with("root=") && !arg.starts_with("zfs="));
                        args.push(format!("{rootprefix}{}", t.dataset));
                    }
                    field(
                        if t.generation.is_some() {
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
    pub fn draw(&mut self, frame: &mut Frame, state: &State, options: &Options) {
        let area = frame.area();
        let colors = Style::default().fg(Color::Gray);
        let regions = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(4),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .split(area);
        let title = options
            .config
            .ui
            .title
            .as_deref()
            .unwrap_or("zbm-rs | ZFS boot environment manager");
        let context = state
            .environments
            .get(state.selected_environment)
            .map(|e| e.dataset.as_str())
            .or_else(|| state.pools.get(state.selected).map(|p| p.name.as_str()))
            .unwrap_or("Select a ZFS pool");
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(
                        clean(title),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("   / {}", View::of(state).title()), colors),
                ]),
                Line::styled(clean(context), colors),
            ]),
            regions[0],
        );
        let wide = area.width >= 110;
        let columns = if wide {
            Layout::horizontal([Constraint::Percentage(48), Constraint::Percentage(52)])
                .split(regions[1])
                .to_vec()
        } else {
            vec![regions[1]]
        };
        let list_area = columns[0];
        let list_regions = Layout::vertical([Constraint::Length(2), Constraint::Min(1)])
            .split(Block::bordered().inner(list_area));
        frame.render_widget(
            Block::bordered()
                .border_style(Style::default().fg(Color::DarkGray))
                .title(View::of(state).title()),
            list_area,
        );
        let query = &self.screens[&self.key].query;
        frame.render_widget(
            Paragraph::new(format!(
                "Find: {}{}",
                clean(query),
                if self.searching { "_" } else { "  [/]" }
            ))
            .style(Style::default().fg(if self.searching {
                Color::Yellow
            } else {
                Color::Gray
            })),
            list_regions[0],
        );
        let rows = self.visible(state);
        let items: Vec<_> = if state.scanning {
            vec![ListItem::new("Discovering pools...")]
        } else if rows.is_empty() {
            vec![ListItem::new(if Self::rows(state).is_empty() {
                "No candidates discovered"
            } else {
                "No targets match this filter"
            })]
        } else {
            rows.iter()
                .map(|row| {
                    ListItem::new(vec![
                        Line::from(vec![
                            Span::raw(clean(&row.title)),
                            Span::styled(
                                format!("  [{}]", row.tag),
                                Style::default().fg(if row.tag.contains("UNAVAILABLE") {
                                    Color::Yellow
                                } else {
                                    Color::Gray
                                }),
                            ),
                        ]),
                        Line::styled(format!("  {}", clean(&row.subtitle)), colors),
                    ])
                })
                .collect()
        };
        frame.render_stateful_widget(
            List::new(items).highlight_symbol("> ").highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
            list_regions[1],
            &mut self.screens.get_mut(&self.key).unwrap().list,
        );
        if wide {
            frame.render_widget(
                Paragraph::new(self.details(state, options))
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::bordered()
                            .border_style(Style::default().fg(Color::DarkGray))
                            .title("Selected target [I] full details"),
                    ),
                columns[1],
            );
        }
        let mut message = status_message(state);
        if state.operation.is_some()
            && let Some(started) = self.operation_started
        {
            message.push_str(&format!(
                "  [{}s / 30s deadline]",
                started.elapsed().as_secs()
            ));
        }
        frame.render_widget(
            Paragraph::new(clean(message))
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(if state.error.is_some() {
                    Color::Yellow
                } else {
                    Color::Gray
                }))
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(Color::DarkGray))
                        .title("Status"),
                ),
            regions[2],
        );
        let mut local = format!("[Enter] {}  [B] Back", self.open_label(state));
        if !state.environments.is_empty() && options.config.ui.show_snapshots {
            local.push_str("  [T] Snapshots");
        }
        if state.snapshot_view {
            local.push_str("  [O] Clone BE  [M] Clone/promote  [U] Rollback");
        }
        if state.snapshot_view && matches!(View::of(state), View::Generations | View::Kernels) {
            local.push_str("  [C] Clone  [D] Discard");
        }
        frame.render_widget(
            Paragraph::new(local)
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(Color::Cyan)),
            regions[3],
        );
        let global = if area.width >= 110 {
            "[F1] Help  [F2] Actions  [I] Details  [R/F5] Rescan  [S/F4] Shell  [N/F6] Restart manager  [P/F10] Power off"
        } else {
            "F1 Help  F2 Actions  I Details  R Rescan  S Shell  N Restart  P Power off"
        };
        frame.render_widget(
            Paragraph::new(global)
                .wrap(Wrap { trim: true })
                .style(colors),
            regions[4],
        );
        if self.panel != Panel::None {
            let width = area.width.saturating_sub(4).min(106);
            let height = area.height.saturating_sub(4).min(30);
            let popup = Rect::new(
                area.x + (area.width - width) / 2,
                area.y + (area.height - height) / 2,
                width,
                height,
            );
            frame.render_widget(Clear, popup);
            let block = Block::bordered()
                .border_style(Style::default().fg(Color::Cyan))
                .title(match self.panel {
                    Panel::Help => "Keyboard help [Esc] close",
                    Panel::Actions => "All actions [Enter] run [Esc] close",
                    Panel::Details => "Full target details [PgUp/PgDn] scroll [Esc] close",
                    Panel::ConfirmDiscard => "Discard prepared clone? [Enter] confirm [Esc] cancel",
                    Panel::ConfirmRollback => "Rollback dataset [Esc] cancel",
                    Panel::ConfirmPromote => "Clone and promote [Enter] confirm [Esc] cancel",
                    Panel::None => unreachable!(),
                });
            if self.panel == Panel::Actions {
                let items: Vec<_> = ALL
                    .into_iter()
                    .map(|command| {
                        let reason = self.reason(command, state, options);
                        ListItem::new(format!(
                            "{:12} {}{}",
                            command.keys(),
                            command.label(),
                            reason
                                .as_ref()
                                .map(|r| format!("  (unavailable: {})", clean(r)))
                                .unwrap_or_default()
                        ))
                        .style(Style::default().fg(if reason.is_some() {
                            Color::Gray
                        } else {
                            Color::Cyan
                        }))
                    })
                    .collect();
                frame.render_stateful_widget(
                    List::new(items)
                        .block(block)
                        .highlight_symbol("> ")
                        .highlight_style(
                            Style::default()
                                .bg(Color::DarkGray)
                                .add_modifier(Modifier::BOLD),
                        ),
                    popup,
                    &mut self.menu,
                );
            } else {
                let text = match self.panel {
                    Panel::Help => {
                        let mut lines: Vec<_> = ALL
                            .into_iter()
                            .map(|command| {
                                Line::raw(format!("{:12} {}", command.keys(), command.label()))
                            })
                            .collect();
                        lines.extend([Line::raw(""),Line::raw("Arrow / Home / End / PgUp / PgDn: select / scroll"),Line::raw("Search: Enter keeps filter; Esc clears filter. Typing never runs commands."),Line::raw("F4 Shell, F6 Restart manager, F10 Power off work during search and operations."),Line::raw("Snapshot: Enter inspects boot targets; O clones BE; M clones/promotes; U rolls back."),Line::raw("Q exits the manager; PID 1 decides recovery. N restarts only the manager.")]);
                        lines
                    }
                    Panel::Details => self.details(state, options),
                    Panel::ConfirmRollback => vec![
                        Line::raw(
                            state
                                .snapshots
                                .get(state.selected_snapshot)
                                .map(|s| clean(&s.source.name))
                                .unwrap_or_default(),
                        ),
                        Line::raw(""),
                        Line::raw(
                            "Discard current changes and ALL newer snapshots of this dataset.",
                        ),
                        Line::raw(
                            "Dependent clones are not forcibly destroyed. Other datasets are not rolled back.",
                        ),
                        Line::raw("Type ROLLBACK and press Enter to confirm. Esc cancels."),
                        Line::raw(format!("> {}_", self.confirmation)),
                    ],
                    Panel::ConfirmPromote => vec![
                        Line::raw(
                            state
                                .snapshots
                                .get(state.selected_snapshot)
                                .map(|s| clean(&s.source.name))
                                .unwrap_or_default(),
                        ),
                        Line::raw("Create an ordinary boot environment and promote it."),
                        Line::raw(
                            "Promotion moves ownership of the origin and older snapshots to the new BE.",
                        ),
                        Line::raw("The source filesystem remains. Enter confirms; Esc cancels."),
                    ],
                    Panel::ConfirmDiscard => vec![
                        Line::raw(
                            self.target(state)
                                .and_then(|t| t.snapshot.as_ref())
                                .map(|s| clean(&s.name))
                                .unwrap_or_default(),
                        ),
                        Line::raw(""),
                        Line::raw("Discard only the prepared owned clone for this snapshot."),
                        Line::raw(
                            "The backend verifies ownership and uses non-recursive destruction.",
                        ),
                        Line::raw("The source dataset and snapshot remain unchanged."),
                        Line::raw(""),
                        Line::raw("Enter: confirm    Esc: cancel"),
                    ],
                    _ => unreachable!(),
                };
                frame.render_widget(
                    Paragraph::new(text)
                        .block(block)
                        .wrap(Wrap { trim: false })
                        .scroll((self.scroll, 0)),
                    popup,
                );
            }
        }
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
                text.contains("Rescan") && text.contains("Shell") && text.contains("Power off"),
                "{width}x{height}: {text}"
            );
            ui.open_panel(Panel::Actions);
            terminal
                .draw(|frame| ui.draw(frame, &state, &options()))
                .unwrap();
            let menu = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                menu.contains("Discard prepared clone")
                    && menu.contains("Exit manager")
                    && menu.contains("Restart manager")
            );
        }
    }

    #[test]
    fn unavailable_generation_is_inspectable_but_never_becomes_a_boot_target() {
        use zbm_core::boot::{BootInputs, GenerationIssue};
        let target = BootTarget {
            dataset: "tank/root".into(),
            generation: Some(1),
            label: "NixOS".into(),
            root: "/run/fixture".into(),
            toplevel: Some("/nix/store/fixture".into()),
            backend: zbm_core::boot::Backend::Nixos,
            snapshot: None,
            mountpoint: None,
            issue: None,
            inputs: BootInputs {
                kernel: "/nix/store/fixture/kernel".into(),
                initrd: None,
                init: Some("/nix/store/fixture/init".into()),
                kernel_params: vec![],
            },
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
                unavailable: Some("Encrypted boot environments are not supported yet".into()),
                properties: BootProperties {
                    mountpoint: "/".into(),
                    canmount: "noauto".into(),
                    encryption: "aes-256-gcm".into(),
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
                && text.contains("Encrypted boot environments")
        );
    }
}
