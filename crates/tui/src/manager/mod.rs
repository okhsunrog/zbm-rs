use crate::session::{Client, Request};
use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;
use std::{
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};
use zbm_core::{State, discover, preview};
mod command;
mod executor;
mod operation;
mod ui;
use command::Command as UiCommand;
use operation::{Action, Outcome, Pending};
use ui::{Input, Panel, Ui};

struct Options {
    preview: bool,
    supervised: bool,
    events: Option<PathBuf>,
    config: crate::config::Config,
}

fn emit(options: &Options, event: &str, state: &State, ui: &Ui) -> Result<()> {
    if let Some(path) = &options.events {
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let value = serde_json::json!({"event": event, "state": state, "pid": std::process::id(), "terminal_size": crossterm::terminal::size().ok(), "config": options.config, "ui": ui.telemetry(state)});
        // Construct a complete record before touching the shared serial device.
        // The leading separator also recovers from another writer's partial line.
        let record = format!("\n{value}\n");
        file.write_all(record.as_bytes())?;
    }
    Ok(())
}

fn status_message(state: &State) -> String {
    if let Some(message) = state.error.as_ref().or(state.operation.as_ref()) {
        return message.clone();
    }
    if let Some(issue) = state.rejected_generations.first() {
        return format!(
            "{} generations unavailable; generation {}: {}. Enter boots a listed generation.",
            state.rejected_generations.len(),
            issue.generation,
            issue.error
        );
    }
    if !state.targets.is_empty() {
        "Enter: boot the selected target. B: back to boot environments."
    } else if state.snapshot_view {
        "Enter: inspect snapshot boot targets. O: clone BE. M: clone/promote. U: rollback. B: back to boot environments."
    } else if !state.environments.is_empty() {
        "Enter: inspect boot targets. * marks pool bootfs. B: back to pools."
    } else {
        "Enter: import selected pool without force and discover boot environments."
    }
    .into()
}

fn refresh(
    terminal: &mut DefaultTerminal,
    state: &mut State,
    options: &Options,
    ui: &mut Ui,
) -> Result<()> {
    ui.sync(state);
    terminal.draw(|frame| ui.draw(frame, state, options))?;
    Ok(())
}

fn save_plan(plan: &zbm_core::boot::BootPlan) -> Result<()> {
    std::fs::write("/run/zbm-rs/boot-plan.json", serde_json::to_vec(plan)?)?;
    Ok(())
}

async fn tui(
    terminal: &mut DefaultTerminal,
    options: &Options,
    channel: Option<&Client>,
) -> Result<()> {
    #[cfg(feature = "vm-test")]
    let control = crate::vm_test::Control::open()?;
    let mut state = State::default();
    let mut ui = Ui::default();
    ui.sync(&mut state);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mut rescan = true;
    let mut pending: Option<Pending> = None;
    let mut last_tick = Instant::now();
    loop {
        #[cfg(feature = "vm-test")]
        if let Some(control) = &control
            && let Some(request) = control.poll(channel)?
        {
            channel
                .ok_or_else(|| anyhow::anyhow!("No supervisor"))?
                .request(request)?;
            break;
        }
        if rescan && !state.scanning && pending.is_none() && !state.requires_restart {
            state.scanning = true;
            rescan = false;
            refresh(terminal, &mut state, options, &mut ui)?;
            emit(options, "scanning", &state, &ui)?;
            let tx = tx.clone();
            let is_preview = options.preview;
            let is_boot = options.supervised;
            tokio::spawn(async move {
                let result = if is_preview {
                    Ok(preview())
                } else if is_boot && !std::path::Path::new("/dev/zfs").exists() {
                    Err("ZFS device unavailable. Inspect the bootstrap log from the recovery shell.".into())
                } else {
                    discover(&zbm_core::Zfs::new()).await
                };
                let _ = tx.send(result).await;
            });
        }
        if let Ok(result) = rx.try_recv() {
            state.apply_scan(result);
            refresh(terminal, &mut state, options, &mut ui)?;
            emit(options, "ready", &state, &ui)?;
        }
        if pending.as_ref().is_some_and(Pending::is_finished) {
            let mut completed = pending.take().unwrap();
            state.operation = None;
            let event = match completed.finish().await {
                Ok(Outcome::Changed {
                    environments,
                    selected,
                    message,
                }) => {
                    state.selected_environment = environments
                        .iter()
                        .position(|e| e.dataset == selected)
                        .unwrap_or(0);
                    state.environments = environments;
                    state.targets.clear();
                    state.rejected_generations.clear();
                    state.snapshots.clear();
                    state.snapshot_view = false;
                    ui.reset_after_mutation();
                    state.error = Some(message);
                    "environment-changed"
                }
                Ok(Outcome::Environments(environments)) => {
                    state.error = environments.is_empty().then(|| {
                        "No visible boot environments (mountpoint=/ or legacy with active=on)"
                            .into()
                    });
                    state.selected_environment = environments
                        .iter()
                        .position(|environment| environment.is_default)
                        .unwrap_or(0);
                    state.environments = environments;
                    "boot-environments"
                }
                Ok(Outcome::Snapshots(snapshots)) => {
                    state.targets.clear();
                    state.rejected_generations.clear();
                    state.snapshots = snapshots;
                    state.selected_snapshot = 0;
                    state.snapshot_view = true;
                    state.error = None;
                    "snapshots"
                }
                Ok(Outcome::Targets(discovery)) => {
                    state.error = discovery
                        .targets
                        .is_empty()
                        .then(|| "No usable boot targets in this snapshot".into());
                    state.targets = discovery.targets;
                    state.rejected_generations = discovery.rejected;
                    state.selected_target = 0;
                    "boot-targets"
                }
                Ok(Outcome::Prepared(plan)) => {
                    ui.prepared = Some(plan.clone());
                    state.error = Some(format!(
                        "Prepared clone: {}. Enter boots it; restart reuses it.",
                        plan.target.dataset
                    ));
                    save_plan(&plan)?;
                    "snapshot-prepared"
                }
                Ok(Outcome::Discarded(dataset)) => {
                    ui.prepared = None;
                    state.error = Some(format!("Discarded prepared clone {dataset}"));
                    "snapshot-discarded"
                }
                Ok(Outcome::Boot(plan, loaded)) => {
                    save_plan(&plan)?;
                    emit(options, "kexec", &state, &ui)?;
                    ratatui::restore();
                    channel.unwrap().request(Request::KexecStarting)?;
                    loaded.execute()?;
                    unreachable!("successful kexec leaves this kernel")
                }
                Err(error) => {
                    state.requires_restart = error.is::<tokio::task::JoinError>()
                        || matches!(completed.error_event, "rollback-error" | "clone-error");
                    state.error = Some(format!("{} failed: {error:#}", completed.description));
                    completed.error_event
                }
            };
            refresh(terminal, &mut state, options, &mut ui)?;
            emit(options, event, &state, &ui)?;
        } else if pending.as_ref().is_some_and(Pending::expired) {
            // Cancellation can interrupt an import/clone/export after an effect.
            // Keep the intent journal and require a fresh manager to reconcile.
            pending.take();
            state.operation = None;
            state.requires_restart = true;
            state.error = Some("Operation timed out after 30 seconds. Use Shell or Restart to reconcile ZFS state.".into());
            refresh(terminal, &mut state, options, &mut ui)?;
            emit(options, "operation-timeout", &state, &ui)?;
        }
        if state.operation.is_some() && last_tick.elapsed() >= Duration::from_secs(1) {
            refresh(terminal, &mut state, options, &mut ui)?;
            last_tick = Instant::now();
        }
        if event::poll(Duration::from_millis(50))? {
            let input = event::read()?;
            if matches!(input, Event::Resize(..)) {
                refresh(terminal, &mut state, options, &mut ui)?;
                emit(options, "resize", &state, &ui)?;
            }
            if let Event::Key(key) = input
                && key.kind == KeyEventKind::Press
            {
                let mut action = None;
                let mut observed = None;
                match ui.input(key, &mut state) {
                    Input::Ignored => {}
                    Input::Changed(event) => observed = Some(event),
                    Input::Command(command, confirmed) => {
                        if let Some(reason) = ui.reason(command, &state, options) {
                            state.error = Some(reason);
                            observed = Some("command-disabled");
                        } else {
                            match command {
                                UiCommand::Exit => {
                                    emit(options, "exit", &state, &ui)?;
                                    break;
                                }
                                UiCommand::Shell => {
                                    emit(options, "shell", &state, &ui)?;
                                    if let Some(channel) = channel {
                                        channel.request(Request::EmergencyShell)?;
                                        break;
                                    }
                                    ratatui::restore();
                                    let result = Command::new("/bin/sh").status();
                                    *terminal = ratatui::init();
                                    result?;
                                    observed = Some("shell-return");
                                }
                                UiCommand::Restart => {
                                    channel.unwrap().request(Request::Restart)?;
                                    break;
                                }
                                UiCommand::PowerOff => {
                                    channel.unwrap().request(Request::PowerOff)?;
                                    break;
                                }
                                UiCommand::Back => {
                                    if !state.targets.is_empty()
                                        || !state.rejected_generations.is_empty()
                                    {
                                        state.targets.clear();
                                        state.rejected_generations.clear();
                                        state.selected_target = 0;
                                    } else if state.snapshot_view {
                                        state.snapshot_view = false;
                                        state.snapshots.clear();
                                    } else {
                                        state.environments.clear();
                                    }
                                    state.error = None;
                                    observed = Some("back");
                                }
                                UiCommand::Snapshots => {
                                    let environment =
                                        &state.environments[state.selected_environment];
                                    action = Some(Action::Snapshots {
                                        pool: state.pools[state.selected].clone(),
                                        dataset: environment.dataset.clone(),
                                    });
                                }
                                UiCommand::Prepare => {
                                    action = Some(Action::Prepare {
                                        pool: state.pools[state.selected].clone(),
                                        target: ui.target(&state).unwrap().clone(),
                                        args: options.config.kernel_args.clone(),
                                    });
                                }
                                UiCommand::Rollback if !confirmed => {
                                    ui.open_panel(Panel::ConfirmRollback);
                                    observed = Some("rollback-confirmation");
                                }
                                UiCommand::ClonePromote if !confirmed => {
                                    ui.open_panel(Panel::ConfirmPromote);
                                    observed = Some("promote-confirmation");
                                }
                                UiCommand::Rollback => {
                                    action = Some(Action::Rollback {
                                        pool: state.pools[state.selected].clone(),
                                        source: state.snapshots[state.selected_snapshot]
                                            .source
                                            .clone(),
                                        limit: options.config.nixos.generation_limit,
                                    });
                                }
                                UiCommand::Clone | UiCommand::ClonePromote => {
                                    action = Some(Action::Clone {
                                        pool: state.pools[state.selected].clone(),
                                        source: state.snapshots[state.selected_snapshot]
                                            .source
                                            .clone(),
                                        promote: command == UiCommand::ClonePromote,
                                        limit: options.config.nixos.generation_limit,
                                    });
                                }
                                UiCommand::Discard if !confirmed => {
                                    ui.open_panel(Panel::ConfirmDiscard);
                                    observed = Some("discard-confirmation");
                                }
                                UiCommand::Discard => {
                                    action = Some(Action::Discard {
                                        pool: state.pools[state.selected].clone(),
                                        source: state.snapshots[state.selected_snapshot]
                                            .source
                                            .clone(),
                                    });
                                }
                                UiCommand::Rescan => rescan = true,
                                UiCommand::Open => {
                                    if let Some(target) = ui.target(&state) {
                                        action = Some(Action::Boot {
                                            pool: state.pools[state.selected].clone(),
                                            target: target.clone(),
                                            args: options.config.kernel_args.clone(),
                                        });
                                    } else if state.snapshot_view {
                                        let snapshot = &state.snapshots[state.selected_snapshot];
                                        action = Some(Action::Targets {
                                            pool: state.pools[state.selected].clone(),
                                            snapshot: snapshot.clone(),
                                            limit: options.config.nixos.generation_limit,
                                        });
                                    } else if !state.environments.is_empty() {
                                        let environment =
                                            &state.environments[state.selected_environment];
                                        state.error = environment.unavailable.clone();
                                        state.targets = environment.targets.clone();
                                        state.rejected_generations =
                                            environment.rejected_generations.clone();
                                        state.selected_target = 0;
                                        observed = Some("boot-targets");
                                    } else {
                                        action = Some(Action::Environments {
                                            pool: state.pools[state.selected].clone(),
                                            readonly: matches!(
                                                options.config.zfs.import_policy,
                                                crate::config::ImportPolicy::ReadOnly
                                            ),
                                            limit: options.config.nixos.generation_limit,
                                        });
                                    }
                                }
                                UiCommand::Search => {
                                    ui.searching = true;
                                    observed = Some("filter");
                                }
                                UiCommand::Help | UiCommand::Actions | UiCommand::Details => {
                                    ui.open_panel(match command {
                                        UiCommand::Help => Panel::Help,
                                        UiCommand::Actions => Panel::Actions,
                                        _ => Panel::Details,
                                    });
                                    observed = Some("panel");
                                }
                            }
                        }
                    }
                }
                if let Some(action) = action {
                    let operation = Pending::start(action);
                    state.operation = Some(format!(
                        "{}. Shell, Restart and Power off remain available.",
                        operation.description
                    ));
                    state.error = None;
                    pending = Some(operation);
                    ui.operation_started = Some(Instant::now());
                    observed = Some("operation-started");
                }
                if let Some(event) = observed {
                    refresh(terminal, &mut state, options, &mut ui)?;
                    emit(options, event, &state, &ui)?;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(())
}

pub fn run(channel: Option<Client>) -> Result<()> {
    let mut options = Options {
        config: crate::config::load(channel.is_some())?,
        preview: false,
        supervised: channel.is_some(),
        events: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--preview" => options.preview = true,
            "--manager" => {}
            "--events" => {
                options.events = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--events requires a path"))?
                        .into(),
                )
            }
            "--help" | "-h" => {
                println!("zbm-rs [--manager] [--preview] [--events PATH]");
                return Ok(());
            }
            _ => anyhow::bail!("Unknown option: {arg}"),
        }
    }
    if options.supervised {
        let _ = Command::new("/usr/bin/modprobe").arg("zfs").status()?;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut terminal = ratatui::init();
    let result = runtime.block_on(tui(&mut terminal, &options, channel.as_ref()));
    ratatui::restore();
    // A blocked filesystem read must not hold a requested recovery session open.
    runtime.shutdown_timeout(Duration::from_millis(100));
    result
}
