use anyhow::{Result, bail};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc};

slint::slint! {
    export { BootManager, BootEntry } from "gui.slint";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    All,
    Nixos,
    Snapshots,
    Linux,
}

impl Source {
    const ALL: [Self; 4] = [Self::All, Self::Nixos, Self::Snapshots, Self::Linux];
    const LABELS: [&str; 4] = [
        "All entries",
        "NixOS generations",
        "Snapshots",
        "Linux kernels",
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|source| *source == self).unwrap()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum EntryId {
    Generation(u32),
    Snapshot(String),
    Kernel(String),
    Recovery,
}

#[derive(Clone)]
struct Entry {
    id: EntryId,
    source: Source,
    view: BootEntry,
}

struct Preview {
    entries: Vec<Entry>,
    source: Source,
    selected: Option<EntryId>,
    status: String,
}

impl Preview {
    fn new(entries: Vec<Entry>) -> Self {
        let selected = entries.first().map(|entry| entry.id.clone());
        Self {
            entries,
            source: Source::All,
            selected,
            status: String::new(),
        }
    }

    fn visible(&self) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|entry| self.source == Source::All || entry.source == self.source)
            .collect()
    }

    fn select_source(&mut self, index: i32) {
        let Some(source) = usize::try_from(index)
            .ok()
            .and_then(|i| Source::ALL.get(i))
            .copied()
        else {
            return;
        };
        if self.source == source {
            return;
        }
        self.source = source;
        let visible = self.visible();
        if !visible
            .iter()
            .any(|entry| Some(&entry.id) == self.selected.as_ref())
        {
            self.selected = visible.first().map(|entry| entry.id.clone());
        }
        self.status.clear();
    }

    fn select_row(&mut self, row: i32) {
        let id = usize::try_from(row)
            .ok()
            .and_then(|row| self.visible().get(row).map(|entry| entry.id.clone()));
        if let Some(id) = id
            && self.selected.as_ref() != Some(&id)
        {
            self.selected = Some(id);
            self.status.clear();
        }
    }

    fn move_selection(&mut self, delta: i32) {
        let visible = self.visible();
        if visible.is_empty() {
            return;
        }
        let row = visible
            .iter()
            .position(|entry| Some(&entry.id) == self.selected.as_ref())
            .unwrap_or(0) as i32;
        self.select_row((row + delta).clamp(0, visible.len() as i32 - 1));
    }

    fn activate(&mut self) {
        if let Some(entry) = self
            .visible()
            .into_iter()
            .find(|entry| Some(&entry.id) == self.selected.as_ref())
        {
            self.status = format!("Preview: {} — no action performed", entry.view.title);
        }
    }

    fn sync(&self, ui: &BootManager) {
        let visible = self.visible();
        ui.set_entries(ModelRc::new(VecModel::from(
            visible
                .iter()
                .map(|entry| entry.view.clone())
                .collect::<Vec<_>>(),
        )));
        self.sync_selection(ui);
    }

    fn sync_selection(&self, ui: &BootManager) {
        let visible = self.visible();
        let row = visible
            .iter()
            .position(|entry| Some(&entry.id) == self.selected.as_ref());
        ui.set_selected_row(row.map_or(-1, |row| row as i32));
        ui.set_selected_entry(row.map(|row| visible[row].view.clone()).unwrap_or_default());
        ui.set_selected_source(self.source.index() as i32);
        ui.set_action_status(self.status.clone().into());
    }
}

fn demo_entries() -> Vec<Entry> {
    [
        (
            EntryId::Generation(42),
            Source::Nixos,
            "NixOS generation 42",
            "2026-10-12 · current system",
            "CURRENT",
            "novafs/nixos",
            false,
        ),
        (
            EntryId::Generation(41),
            Source::Nixos,
            "NixOS generation 41",
            "2026-10-10 · previous system",
            "SIGNED",
            "novafs/nixos",
            false,
        ),
        (
            EntryId::Snapshot("before-kernel-update".into()),
            Source::Snapshots,
            "@before-kernel-update",
            "Snapshot · writable clone on boot",
            "TRUSTED",
            "novafs/nixos@before-kernel-update",
            false,
        ),
        (
            EntryId::Snapshot("known-good".into()),
            Source::Snapshots,
            "@known-good",
            "Snapshot · 3 days ago",
            "TRUSTED",
            "novafs/nixos@known-good",
            false,
        ),
        (
            EntryId::Kernel("6.17.1-zfs".into()),
            Source::Linux,
            "Linux 6.17.1-zfs",
            "Generic Linux boot target",
            "SIGNED",
            "novafs/nixos",
            false,
        ),
        (
            EntryId::Recovery,
            Source::All,
            "Recovery tools",
            "Diagnostics and reboot controls",
            "LOCAL",
            "Not applicable",
            true,
        ),
    ]
    .into_iter()
    .map(
        |(id, source, title, subtitle, badge, dataset, recovery)| Entry {
            id,
            source,
            view: BootEntry {
                title: title.into(),
                subtitle: subtitle.into(),
                badge: badge.into(),
                dataset: dataset.into(),
                recovery,
                verification: if recovery {
                    "Demo: local recovery entry"
                } else {
                    "Demo: kernel, initramfs and arguments authorized"
                }
                .into(),
                measurement: if recovery {
                    "Not applicable"
                } else {
                    "Demo: prepared for PCR 15"
                }
                .into(),
            },
        },
    )
    .collect()
}

// Fixtures are confined to the GUI prototype; they never discover or change host state.
fn fixture_entries(fixture: &str) -> Result<Vec<Entry>> {
    let mut entries = demo_entries();
    match fixture {
        "demo" => {}
        "empty" => entries.clear(),
        "long" => {
            entries[0].view.title =
                "NixOS generation 42 — workstation with a very long configuration name".into();
            entries[0].view.subtitle =
                "2026-10-12 · configuration with additional recovery and development tools".into();
            entries[0].view.dataset = "novafs/hosts/workstation-with-a-long-hostname/nixos@before-kernel-update-and-driver-migration".into();
        }
        "many" => {
            let template = entries[0].clone();
            entries = (1..=60)
                .rev()
                .map(|generation| {
                    let mut entry = template.clone();
                    entry.id = EntryId::Generation(generation);
                    entry.view.title = format!("NixOS generation {generation}").into();
                    entry.view.badge = "SIGNED".into();
                    entry.view.subtitle =
                        "Sample generation · scroll to inspect older entries".into();
                    entry
                })
                .collect();
        }
        _ => bail!("Unknown GUI fixture {fixture:?}; expected demo, empty, long or many"),
    }
    Ok(entries)
}

pub fn run() -> Result<()> {
    slint::BackendSelector::new().select()?;
    let ui = BootManager::new()?;
    ui.set_sources(ModelRc::new(VecModel::from(
        Source::LABELS
            .iter()
            .map(|label| (*label).into())
            .collect::<Vec<_>>(),
    )));
    let state = Rc::new(RefCell::new(Preview::new(fixture_entries(
        &std::env::var("ZBM_GUI_FIXTURE").unwrap_or_else(|_| "demo".into()),
    )?)));
    state.borrow().sync(&ui);
    let weak = ui.as_weak();
    let controller = state.clone();
    ui.on_select_source(move |index| {
        if let Some(ui) = weak.upgrade() {
            let mut state = controller.borrow_mut();
            state.select_source(index);
            state.sync(&ui);
        }
    });
    let weak = ui.as_weak();
    let controller = state.clone();
    ui.on_select_row(move |row| {
        if let Some(ui) = weak.upgrade() {
            let mut state = controller.borrow_mut();
            state.select_row(row);
            state.sync_selection(&ui);
        }
    });
    let weak = ui.as_weak();
    let controller = state.clone();
    ui.on_move_selection(move |delta| {
        if let Some(ui) = weak.upgrade() {
            let mut state = controller.borrow_mut();
            state.move_selection(delta);
            state.sync_selection(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_activate(move || {
        if let Some(ui) = weak.upgrade() {
            let mut state = state.borrow_mut();
            state.activate();
            ui.set_action_status(state.status.clone().into());
        }
    });
    ui.run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_and_navigation_share_visible_rows() {
        let mut state = Preview::new(demo_entries());
        state.select_source(2);
        assert_eq!(state.visible().len(), 2);
        assert_eq!(
            state.selected,
            Some(EntryId::Snapshot("before-kernel-update".into()))
        );
        state.move_selection(1);
        state.move_selection(1);
        assert_eq!(state.selected, Some(EntryId::Snapshot("known-good".into())));
        state.move_selection(-100);
        assert_eq!(
            state.selected,
            Some(EntryId::Snapshot("before-kernel-update".into()))
        );
    }

    #[test]
    fn selection_keeps_identity_when_filter_changes_its_position() {
        let mut state = Preview::new(demo_entries());
        state.select_row(3);
        state.select_source(2);
        assert_eq!(state.selected, Some(EntryId::Snapshot("known-good".into())));
        assert_eq!(state.visible()[1].id, state.selected.clone().unwrap());
    }

    #[test]
    fn changing_target_or_source_clears_action_result() {
        let mut state = Preview::new(demo_entries());
        state.activate();
        assert!(state.status.contains("generation 42"));
        state.select_row(1);
        assert!(state.status.is_empty());
        state.activate();
        state.select_source(1);
        assert!(state.status.is_empty());
        state.select_source(2);
        state.activate();
        assert!(state.status.contains("before-kernel-update"));
    }

    #[test]
    fn empty_and_invalid_selection_cannot_activate_an_entry() {
        let mut state = Preview::new(vec![]);
        state.move_selection(1);
        state.select_row(-1);
        state.select_row(12);
        state.select_source(10);
        state.activate();
        assert!(state.selected.is_none());
        assert!(state.status.is_empty());
        let mut state = Preview::new(demo_entries());
        state.select_row(99);
        assert_eq!(state.selected, Some(EntryId::Generation(42)));
    }

    #[test]
    fn large_fixture_navigates_to_the_last_entry() {
        let mut state = Preview::new(fixture_entries("many").unwrap());
        state.move_selection(100);
        assert_eq!(state.selected, Some(EntryId::Generation(1)));
        state.select_source(2);
        assert!(state.visible().is_empty());
        assert!(state.selected.is_none());
        state.activate();
        assert!(state.status.is_empty());
    }
}
