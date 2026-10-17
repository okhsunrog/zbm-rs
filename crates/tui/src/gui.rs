//! Slint is a presentation adapter for the same manager used by the TUI.
mod preview;
use crate::manager::{
    Frontend, Options,
    ui::{Panel, Ui},
};
use crate::session::Client;
use anyhow::{Result, anyhow, ensure};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

slint::slint! { export { BootManager, BootEntry, ManagerAction } from "gui.slint"; }

pub(crate) struct Row {
    pub title: String,
    pub subtitle: String,
    pub badge: String,
    pub unavailable: bool,
}
pub(crate) struct AvailableAction {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub reason: String,
}
pub(crate) struct Snapshot {
    pub query: String,
    pub revision: i32,
    pub heading: String,
    pub selected: i32,
    pub rows: Vec<Row>,
    pub details: String,
    pub status: String,
    pub primary: String,
    pub primary_enabled: bool,
    pub actions: Vec<AvailableAction>,
    pub panel: Panel,
    pub confirmation: String,
    pub security: String,
    pub editable: bool,
}
// Never derive Debug/Serialize: Unlock contains a secret.
pub(crate) enum Event {
    Select {
        revision: i32,
        row: i32,
    },
    Move {
        revision: i32,
        delta: i32,
    },
    Filter {
        revision: i32,
        text: String,
    },
    Command {
        revision: i32,
        id: i32,
    },
    Unlock {
        revision: i32,
        passphrase: zeroize::Zeroizing<Vec<u8>>,
    },
    Confirm {
        revision: i32,
        text: String,
    },
    Cancel {
        revision: i32,
    },
}
impl Event {
    pub fn revision(&self) -> i32 {
        match self {
            Self::Select { revision, .. }
            | Self::Move { revision, .. }
            | Self::Filter { revision, .. }
            | Self::Command { revision, .. }
            | Self::Unlock { revision, .. }
            | Self::Confirm { revision, .. }
            | Self::Cancel { revision } => *revision,
        }
    }
}

pub(crate) struct Transport {
    ui: slint::Weak<BootManager>,
    events: mpsc::Receiver<Event>,
    stop: Arc<AtomicBool>,
}
impl Transport {
    pub fn draw(&self, ui: &mut Ui, state: &zbm_core::State, options: &Options) -> Result<()> {
        let snapshot = ui.gui_snapshot(state, options);
        self.ui
            .upgrade_in_event_loop(move |ui| snapshot.apply(&ui))?;
        Ok(())
    }
    pub fn poll(&self) -> Result<Option<Event>> {
        ensure!(!self.stop.load(Ordering::Acquire), "GUI event loop stopped");
        match self.events.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(anyhow!("GUI input disconnected")),
        }
    }
    pub fn hide(&self) -> Result<()> {
        // Hide where supported; PID 1 owns device/VT restoration after exit.
        self.ui.upgrade_in_event_loop(|ui| {
            let _ = ui.hide();
        })?;
        Ok(())
    }
}
impl Snapshot {
    fn apply(self, ui: &BootManager) {
        let rows: Vec<_> = self
            .rows
            .into_iter()
            .map(|r| BootEntry {
                title: r.title.into(),
                subtitle: r.subtitle.into(),
                badge: r.badge.into(),
                recovery: r.unavailable,
                ..Default::default()
            })
            .collect();
        ui.set_selected_entry(
            usize::try_from(self.selected)
                .ok()
                .and_then(|i| rows.get(i))
                .cloned()
                .unwrap_or_default(),
        );
        if !ui.get_entries().iter().eq(rows.iter().cloned()) {
            ui.set_entries(ModelRc::new(VecModel::from(rows)));
        }
        ui.set_selected_row(self.selected);
        ui.set_filter_text(self.query.into());
        ui.set_heading(self.heading.into());
        ui.set_details_text(self.details.into());
        ui.set_action_status(self.status.into());
        ui.set_primary_label(self.primary.into());
        ui.set_primary_enabled(self.primary_enabled);
        ui.set_security_status(self.security.into());
        let became_editable = !ui.get_editable() && self.editable;
        ui.set_editable(self.editable);
        let actions: Vec<_> = self
            .actions
            .into_iter()
            .map(|a| ManagerAction {
                id: a.id,
                label: a.label.into(),
                enabled: a.enabled,
                reason: a.reason.into(),
            })
            .collect();
        // Replacing an unchanged model destroys repeated controls and their focus.
        if !ui.get_actions().iter().eq(actions.iter().cloned()) {
            ui.set_actions(ModelRc::new(VecModel::from(actions)));
        }
        let modal = !self.confirmation.is_empty();
        ui.set_unlock_prompt(self.panel == Panel::Unlock);
        ui.set_rollback_prompt(self.panel == Panel::ConfirmRollback);
        ui.set_confirmation_text(self.confirmation.into());
        let was_modal = ui.get_modal_open();
        ui.set_modal_open(modal);
        ui.set_revision(self.revision);
        if modal && !was_modal {
            ui.invoke_focus_prompt();
        }
        if !modal && (was_modal || became_editable) {
            ui.invoke_focus_entries();
        }
    }
}

fn command_id(command: crate::manager::command::Command) -> i32 {
    crate::manager::command::ALL
        .iter()
        .position(|c| *c == command)
        .unwrap() as i32
}

fn callbacks(ui: &BootManager, tx: mpsc::SyncSender<Event>) {
    macro_rules! connect {
        ($method:ident, |$ui:ident $(, $arg:ident)*| $event:expr) => {{
            let weak = ui.as_weak(); let tx = tx.clone();
            ui.$method(move |$($arg),*| { if let Some($ui) = weak.upgrade() { let _ = tx.try_send($event); } });
        }};
    }
    connect!(on_select_row, |ui, row| Event::Select {
        revision: ui.get_revision(),
        row
    });
    connect!(on_move_selection, |ui, delta| Event::Move {
        revision: ui.get_revision(),
        delta
    });
    connect!(on_filter_changed, |ui, text| Event::Filter {
        revision: ui.get_revision(),
        text: text.to_string()
    });
    connect!(on_activate, |ui| Event::Command {
        revision: ui.get_revision(),
        id: command_id(crate::manager::command::Command::Open)
    });
    let weak = ui.as_weak();
    let shortcuts = tx.clone();
    ui.on_shortcut(move |text| {
        use crate::manager::command::Command;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let code = match text.as_str() {
            "F4" => KeyCode::F(4),
            "F5" => KeyCode::F(5),
            "F6" => KeyCode::F(6),
            "F10" => KeyCode::F(10),
            text if text.len() == 1 => KeyCode::Char(text.chars().next().unwrap()),
            _ => return,
        };
        if let Some(command) = Command::from_key(KeyEvent::new(code, KeyModifiers::NONE))
            && !matches!(
                command,
                Command::Help | Command::Details | Command::Search | Command::Actions
            )
            && let Some(ui) = weak.upgrade()
        {
            let _ = shortcuts.try_send(Event::Command {
                revision: ui.get_revision(),
                id: command_id(command),
            });
        }
    });
    connect!(on_command, |ui, id| Event::Command {
        revision: ui.get_revision(),
        id
    });
    connect!(on_unlock, |ui, text| Event::Unlock {
        revision: ui.get_revision(),
        passphrase: zeroize::Zeroizing::new(text.as_bytes().to_vec())
    });
    connect!(on_confirm, |ui, text| Event::Confirm {
        revision: ui.get_revision(),
        text: text.to_string()
    });
    connect!(on_cancel, |ui| Event::Cancel {
        revision: ui.get_revision()
    });
    let weak = ui.as_weak();
    ui.window().on_close_requested(move || {
        if let Some(ui) = weak.upgrade() {
            let _ = tx.try_send(Event::Command {
                revision: ui.get_revision(),
                id: command_id(crate::manager::command::Command::Exit),
            });
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
}

pub(crate) fn run(
    options: Options,
    channel: Option<Client>,
    #[cfg(feature = "vm-test")] control: Option<crate::vm_test::Control>,
) -> Result<()> {
    if options.preview {
        ensure!(
            !options.supervised,
            "Preview must not replace a supervised manager"
        );
        return preview::run();
    }
    ensure!(
        options.supervised,
        "Use --preview for host UI review; live GUI requires the PID 1 supervisor"
    );
    slint::BackendSelector::new().select()?;
    let ui = BootManager::new()?;
    ui.set_live(true);
    ui.set_action_status("Starting boot manager…".into());
    let (tx, events) = mpsc::sync_channel(64);
    let stop = Arc::new(AtomicBool::new(false));
    let worker = std::rc::Rc::new(std::cell::RefCell::new(None));
    let startup_error = std::rc::Rc::new(std::cell::RefCell::new(None));
    // LinuxKMS opens DRM, libinput and its VT in event-loop initialization.
    // No action callbacks or manager workers exist until privileges are dropped.
    let weak = ui.as_weak();
    let worker_slot = worker.clone();
    let error_slot = startup_error.clone();
    let worker_stop = stop.clone();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        let startup = (|| -> Result<()> {
            if options.config.security.mode == crate::config::SecurityMode::Enforce {
                crate::broker::start()?;
            }
            let ui = weak
                .upgrade()
                .ok_or_else(|| anyhow!("GUI disappeared during startup"))?;
            callbacks(&ui, tx);
            ui.invoke_focus_entries();
            let transport = Transport {
                ui: ui.as_weak(),
                events,
                stop: worker_stop,
            };
            *worker_slot.borrow_mut() = Some(std::thread::spawn(move || {
                let result = (|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    let result = runtime.block_on(crate::manager::drive(
                        &mut Frontend::Gui(transport),
                        &options,
                        channel.as_ref(),
                        #[cfg(feature = "vm-test")]
                        control.as_ref(),
                    ));
                    runtime.shutdown_timeout(std::time::Duration::from_millis(100));
                    result
                })();
                let _ = slint::quit_event_loop();
                result
            }));
            Ok(())
        })();
        if let Err(error) = startup {
            *error_slot.borrow_mut() = Some(error);
            let _ = slint::quit_event_loop();
        }
    });
    let result = ui.run();
    stop.store(true, Ordering::Release);
    let worker_result = worker
        .borrow_mut()
        .take()
        .map(|w| w.join().map_err(|_| anyhow!("Manager worker panicked")))
        .transpose()?
        .transpose();
    if let Some(error) = startup_error.borrow_mut().take() {
        return Err(error);
    }
    result?;
    worker_result?;
    Ok(())
}
