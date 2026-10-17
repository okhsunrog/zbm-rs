use crate::session::{Client, Request};
use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    process::Command,
    time::{Duration, Instant},
};
use zbm_core::{State, discover, preview};
mod command;
pub(crate) mod executor;
pub(crate) mod operation;
mod ui;
use command::Command as UiCommand;
use operation::{Action, Outcome, Workflow};
use ui::{Input, Panel, Ui};

struct Options {
    preview: bool,
    supervised: bool,
    events: Option<File>,
    config: crate::config::Config,
}

fn emit(options: &Options, event: &str, state: &State, ui: &Ui) -> Result<()> {
    if let Some(mut file) = options.events.as_ref() {
        let value = serde_json::json!({"event": event, "state": state, "pid": std::process::id(), "terminal_size": crossterm::terminal::size().ok(), "config": options.config, "ui": ui.telemetry(state)});
        // Construct a complete record before touching the shared serial device.
        // The leading separator also recovers from another writer's partial line.
        let record = format!("\n{value}\n");
        file.write_all(record.as_bytes())?;
    }
    Ok(())
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
    #[cfg(feature = "vm-test")] control: Option<&crate::vm_test::Control>,
) -> Result<()> {
    let mut state = State::default();
    let mut ui = Ui::default();
    ui.sync(&mut state);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mut rescan = true;
    let mut workflow = Workflow::default();
    let mut last_tick = Instant::now();
    loop {
        #[cfg(feature = "vm-test")]
        if let Some(control) = control
            && let Some(request) = control.poll(channel)?
        {
            channel
                .ok_or_else(|| anyhow::anyhow!("No supervisor"))?
                .request(request)?;
            break;
        }
        if rescan && workflow.start_scan() {
            state.activity = workflow.view();
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
                    if crate::broker::enabled() {
                        crate::broker::discover().await
                    } else {
                        discover(&zbm_core::Zfs::new()).await
                    }
                };
                let _ = tx.send(result).await;
            });
        }
        if let Ok(result) = rx.try_recv() {
            workflow.finish_scan();
            state.activity = workflow.view();
            state.apply_scan(result);
            refresh(terminal, &mut state, options, &mut ui)?;
            emit(options, "ready", &state, &ui)?;
        }
        if let Some(mut completed) = workflow.take_finished() {
            state.activity = workflow.view();
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
                    state.error = None;
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
                    workflow.recover(error.recovery);
                    state.activity = workflow.view();
                    state.error = Some(format!(
                        "{} failed: {:#}",
                        completed.description, error.source
                    ));
                    completed.error_event
                }
            };
            refresh(terminal, &mut state, options, &mut ui)?;
            emit(options, event, &state, &ui)?;
        } else if workflow.expire() {
            // Cancellation can interrupt an import/clone/export after an effect.
            // Keep the intent journal and require a fresh manager to reconcile.
            state.activity = workflow.view();
            state.error = Some("Operation timed out after 30 seconds. Use Shell or Restart to reconcile ZFS state.".into());
            refresh(terminal, &mut state, options, &mut ui)?;
            emit(options, "operation-timeout", &state, &ui)?;
        }
        if state.activity.operation().is_some() && last_tick.elapsed() >= Duration::from_secs(1) {
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
                    Input::Unlock(passphrase) => {
                        if let Some(reason) = ui.reason(UiCommand::Open, &state, options) {
                            state.error = Some(reason);
                            observed = Some("command-disabled");
                        } else if let Some(environment) =
                            state.environments.get(state.selected_environment)
                            && environment.properties.can_unlock()
                        {
                            action = Some(Action::Unlock {
                                pool: state.pools[state.selected].clone(),
                                dataset: environment.dataset.clone(),
                                passphrase,
                                readonly: matches!(
                                    options.config.zfs.import_policy,
                                    crate::config::ImportPolicy::ReadOnly
                                ),
                                limit: options.config.nixos.generation_limit,
                            });
                        }
                    }
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
                                        if environment.properties.can_unlock() {
                                            ui.open_panel(Panel::Unlock);
                                            state.error = None;
                                            refresh(terminal, &mut state, options, &mut ui)?;
                                            emit(options, "unlock-prompt", &state, &ui)?;
                                            continue;
                                        }
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
                    workflow.start(action)?;
                    state.activity = workflow.view();
                    state.error = None;
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
                let path = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--events requires a path"))?;
                // Keep the output descriptor across privilege dropping; the UI
                // never needs to reopen a root-owned serial device or directory.
                options.events = Some(OpenOptions::new().create(true).append(true).open(path)?);
            }
            "--help" | "-h" => {
                println!("zbm-rs [--manager] [--preview] [--events PATH]");
                return Ok(());
            }
            _ => anyhow::bail!("Unknown option: {arg}"),
        }
    }
    #[cfg(feature = "vm-test")]
    let control = crate::vm_test::Control::open()?;
    if options.supervised {
        let loaded = Command::new("/usr/bin/modprobe").arg("zfs").status()?;
        if options.config.security.mode == crate::config::SecurityMode::Enforce {
            anyhow::ensure!(loaded.success(), "Trusted ZFS module could not be loaded");
            crate::broker::start()?;
        }
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut terminal = ratatui::init();
    let result = runtime.block_on(tui(
        &mut terminal,
        &options,
        channel.as_ref(),
        #[cfg(feature = "vm-test")]
        control.as_ref(),
    ));
    ratatui::restore();
    // A blocked filesystem read must not hold a requested recovery session open.
    runtime.shutdown_timeout(Duration::from_millis(100));
    result
}
