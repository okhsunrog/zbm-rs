use crate::session::{Client, Request};
use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;
use std::{
    collections::VecDeque,
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

mod navigation;
use navigation::{Intent, UnlockSelection, intent_action, merge, preferred, readonly};

struct Options {
    preview: bool,
    supervised: bool,
    events: crate::diagnostics::Events,
    config: crate::config::Config,
}

fn emit(options: &Options, event: &str, state: &State, ui: &Ui) -> Result<()> {
    let value = serde_json::json!({"event": event, "uptime": crate::diagnostics::timestamp(), "state": state, "pid": std::process::id(), "terminal_size": crossterm::terminal::size().ok(), "config": options.config, "ui": ui.telemetry(state)});
    // The leading separator also recovers from another writer's partial line.
    options.events.write(format!("\n{value}\n").as_bytes());
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
    if options.supervised && !options.preview {
        state.resume = crate::resume::display_status(&options.config).await;
    }
    let mut ui = Ui::default();
    ui.sync(&mut state);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mut rescan = true;
    let mut workflow = Workflow::default();
    let mut last_tick = Instant::now();
    let mut queue = VecDeque::new();
    let mut inspecting = None;
    let mut catalog_active = false;
    let mut startup_done = false;
    let mut unlocking: Option<UnlockSelection> = None;
    let mut resume_unlock = false;
    let mut auto_cancelled = false;
    let mut auto_deadline: Option<Instant> = None;
    let mut after_cancellation: Option<UiCommand> = None;
    let mut continuation: Option<UiCommand> = None;
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
            if options.supervised && !options.preview {
                state.resume = crate::resume::display_status(&options.config).await;
            }
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
            queue = if options.preview {
                VecDeque::new()
            } else {
                state.pools.clone().into()
            };
            catalog_active = !options.preview;
            refresh(terminal, &mut state, options, &mut ui)?;
            emit(options, "ready", &state, &ui)?;
        }
        if let Some(mut completed) = workflow.take_finished() {
            state.activity = workflow.view();
            let event = match completed.finish().await {
                Ok(Outcome::ResumeCancelled) => {
                    state.resume = crate::resume::display_status(&options.config).await;
                    if state.resume.protected() {
                        after_cancellation = None;
                        state.error =
                            Some("Saved session cancellation could not be verified".into());
                    } else {
                        continuation = after_cancellation.take();
                        ui.resume_confirmation = None;
                        state.error = None;
                    }
                    "resume-cancelled"
                }
                Ok(Outcome::Unlock(zbm_core::unlock::Status::Passphrase(challenge))) => {
                    ui.unlock_prompt = Some(format!(
                        "{}{}",
                        challenge.encryption_root,
                        challenge
                            .reason
                            .as_ref()
                            .map(|reason| format!("\n{reason}"))
                            .unwrap_or_default()
                    ));
                    if let Some(selection) = &mut unlocking {
                        selection.challenge = Some(challenge);
                    }
                    ui.open_panel(Panel::Unlock);
                    "unlock-prompt"
                }
                Ok(Outcome::Unlock(zbm_core::unlock::Status::Ready)) => {
                    state.resume = crate::resume::display_status(&options.config).await;
                    queue = state.pools.clone().into();
                    catalog_active = true;
                    resume_unlock = true;
                    "unlocked"
                }
                Ok(Outcome::Changed {
                    environments,
                    selected,
                    message,
                }) => {
                    state.selected_environment = environments
                        .iter()
                        .position(|e| e.dataset == selected)
                        .unwrap_or(0);
                    let guid = environments
                        .first()
                        .map(|e| e.pool_guid)
                        .unwrap_or_else(|| state.pools[state.selected].guid);
                    merge(&mut state, guid, environments, options);
                    state.targets.clear();
                    state.rejected_generations.clear();
                    state.snapshots.clear();
                    state.snapshot_view = false;
                    ui.reset_after_mutation();
                    state.error = Some(message);
                    "environment-changed"
                }
                Ok(Outcome::Environments(environments)) => {
                    let guid = inspecting
                        .take()
                        .unwrap_or_else(|| state.pools[state.selected].guid);
                    state.pool_errors.remove(&guid);
                    merge(&mut state, guid, environments, options);
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
                    after_cancellation = None;
                    if options.config.resume.enabled() {
                        state.resume = crate::resume::display_status(&options.config).await;
                    }
                    workflow.recover(error.recovery);
                    state.activity = workflow.view();
                    state.error = Some(format!(
                        "{} failed: {:#}",
                        completed.description, error.source
                    ));
                    if let Some(guid) = inspecting.take() {
                        state.pool_errors.insert(guid, state.error.clone().unwrap());
                    }
                    if completed.error_event == "unlock-error" {
                        if unlocking
                            .as_ref()
                            .is_some_and(|selection| selection.challenge.is_some())
                        {
                            ui.open_panel(Panel::Unlock);
                        } else {
                            unlocking = None;
                            auto_cancelled = true;
                        }
                    } else {
                        auto_cancelled = true;
                    }
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
        if !state.activity.is_scanning()
            && state.activity.operation().is_none()
            && !state.activity.requires_restart()
            && catalog_active
        {
            if let Some(pool) = queue.pop_front() {
                inspecting = Some(pool.guid);
                workflow.start(Action::Environments {
                    pool,
                    readonly: readonly(options),
                    limit: options.config.nixos.generation_limit,
                })?;
                state.activity = workflow.view();
            } else {
                catalog_active = false;
                state.error = None;
                let mut action = None;
                if resume_unlock {
                    resume_unlock = false;
                    if let Some(selection) = unlocking.take() {
                        if matches!(selection.intent, Intent::Menu) {
                            if let Some(index) = preferred(&state, options) {
                                ui.focus_environment(&mut state, index);
                            }
                        } else if let Some(index) = state.environments.iter().position(|e| {
                            e.pool_guid == selection.pool.guid && e.dataset == selection.dataset
                        }) {
                            ui.focus_environment(&mut state, index);
                            action = intent_action(
                                selection.intent,
                                &mut state,
                                &mut ui,
                                options,
                                &mut unlocking,
                            );
                        } else {
                            state.error =
                                Some("Selected environment disappeared after unlock".into());
                            auto_cancelled = true;
                        }
                    }
                }
                if !startup_done {
                    startup_done = true;
                    let selected = preferred(&state, options);
                    if let Some(index) = selected {
                        ui.focus_environment(&mut state, index);
                    }
                    let roots: std::collections::BTreeSet<_> = state
                        .environments
                        .iter()
                        .filter(|e| e.properties.locked())
                        .filter_map(|e| {
                            e.properties
                                .encryptionroot
                                .as_ref()
                                .map(|root| (e.pool_guid, root.clone()))
                        })
                        .collect();
                    if let Some((guid, root)) = state.resume.unlock_target()
                        && let Some(pool) = state.pools.iter().find(|p| p.guid == guid).cloned()
                    {
                        let dataset = root.to_owned();
                        unlocking = Some(UnlockSelection {
                            pool: pool.clone(),
                            dataset: dataset.clone(),
                            intent: Intent::Menu,
                            challenge: None,
                        });
                        action = Some(Action::Unlock {
                            pool,
                            dataset,
                            answer: None,
                            readonly: true,
                            limit: options.config.nixos.generation_limit,
                        });
                    } else if (selected.is_some() || roots.len() == 1)
                        && state
                            .environments
                            .get(state.selected_environment)
                            .is_some_and(|e| e.properties.locked())
                    {
                        action = intent_action(
                            Intent::Menu,
                            &mut state,
                            &mut ui,
                            options,
                            &mut unlocking,
                        );
                    }
                }
                if let Some(action) = action {
                    workflow.start(action)?;
                    state.activity = workflow.view();
                }
                refresh(terminal, &mut state, options, &mut ui)?;
                emit(options, "catalog-ready", &state, &ui)?;
            }
        }
        if state.activity.operation().is_some() && last_tick.elapsed() >= Duration::from_secs(1) {
            refresh(terminal, &mut state, options, &mut ui)?;
            last_tick = Instant::now();
        }
        let continued = continuation.take();
        if continued.is_some() || event::poll(Duration::from_millis(50))? {
            let input = if continued.is_some() {
                Event::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Enter,
                    crossterm::event::KeyModifiers::NONE,
                ))
            } else {
                event::read()?
            };
            if matches!(input, Event::Resize(..)) {
                refresh(terminal, &mut state, options, &mut ui)?;
                emit(options, "resize", &state, &ui)?;
            }
            if let Event::Key(key) = input
                && key.kind == KeyEventKind::Press
            {
                let cancelled_timer = state.countdown_secs.is_some() && ui.panel != Panel::Unlock;
                if key.code == crossterm::event::KeyCode::Esc || ui.panel != Panel::Unlock {
                    auto_cancelled = true;
                    auto_deadline = None;
                    state.countdown_secs = None;
                }
                if key.code == crossterm::event::KeyCode::Esc && ui.panel == Panel::Unlock {
                    unlocking = None;
                }
                let mut action = None;
                let mut observed = cancelled_timer.then_some("autoboot-cancelled");
                let input_panel = ui.panel;
                let input = continued
                    .map(|command| Input::Command(command, true))
                    .unwrap_or_else(|| ui.input(key, &mut state));
                match input {
                    Input::Unlock(passphrase) => {
                        if let Some(selection) = &unlocking
                            && let Some(challenge) = &selection.challenge
                        {
                            action = Some(Action::Unlock {
                                pool: selection.pool.clone(),
                                dataset: selection.dataset.clone(),
                                answer: Some(zbm_core::unlock::Answer {
                                    challenge: challenge.clone(),
                                    passphrase,
                                }),
                                readonly: readonly(options),
                                limit: options.config.nixos.generation_limit,
                            });
                        }
                    }
                    Input::Ignored => {}
                    Input::Changed(event) => observed = Some(event),
                    Input::Command(command, confirmed) => {
                        if crate::diagnostics::continuous() {
                            eprintln!(
                                "[{}] ui-command={command:?} panel={input_panel:?} confirmed={confirmed}",
                                crate::diagnostics::timestamp()
                            );
                        }
                        if let Some(reason) = ui.reason(command, &state, options) {
                            state.error = Some(reason);
                            observed = Some("command-disabled");
                        } else if ui.cancels_resume(command, &state) {
                            if !confirmed {
                                ui.confirm_resume(command, &state);
                                observed = Some("resume-cancel-confirmation");
                            } else if let Some((id, expected_command)) = &ui.resume_confirmation {
                                if *expected_command == command
                                    && state.resume.session_id() == Some(id.as_str())
                                {
                                    after_cancellation = Some(command);
                                    action = Some(Action::CancelResume {
                                        session_id: id.clone(),
                                    });
                                } else {
                                    state.error = Some(
                                        "Saved session changed; select the action again".into(),
                                    );
                                    ui.resume_confirmation = None;
                                }
                            } else {
                                state.error =
                                    Some("Saved session cancellation requires confirmation".into());
                            }
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
                                    if state.pool_view {
                                        state.pool_view = false;
                                    } else if !state.targets.is_empty()
                                        || !state.rejected_generations.is_empty()
                                    {
                                        state.targets.clear();
                                        state.rejected_generations.clear();
                                        state.selected_target = 0;
                                    } else if state.snapshot_view {
                                        state.snapshot_view = false;
                                        state.snapshots.clear();
                                    } else {
                                        state.pool_view = false;
                                    }
                                    state.error = None;
                                    observed = Some("back");
                                }
                                UiCommand::Pools => {
                                    state.pool_view = !state.pool_view;
                                    observed = Some("pool-view");
                                }
                                UiCommand::Kernels | UiCommand::Snapshots => {
                                    let intent = if command == UiCommand::Kernels {
                                        Intent::Kernels
                                    } else {
                                        Intent::Snapshots
                                    };
                                    action = intent_action(
                                        intent,
                                        &mut state,
                                        &mut ui,
                                        options,
                                        &mut unlocking,
                                    );
                                    observed = Some("navigation");
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
                                    if state.pool_view {
                                        state.targets.clear();
                                        state.rejected_generations.clear();
                                        state.snapshot_view = false;
                                        action = Some(Action::Environments {
                                            pool: state.pools[state.selected].clone(),
                                            readonly: readonly(options),
                                            limit: options.config.nixos.generation_limit,
                                        });
                                    } else if let Some(target) = ui.target(&state) {
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
                                    } else if !state.environments.is_empty() && !state.pool_view {
                                        action = intent_action(
                                            Intent::Boot,
                                            &mut state,
                                            &mut ui,
                                            options,
                                            &mut unlocking,
                                        );
                                        observed = Some("boot-selection");
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
                    if let Action::Environments { pool, .. } = &action {
                        inspecting = Some(pool.guid);
                        state.pool_view = false;
                    }
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
        if startup_done
            && !auto_cancelled
            && !catalog_active
            && unlocking.is_none()
            && state.activity.operation().is_none()
            && !state.activity.requires_restart()
            && ui.panel == Panel::None
        {
            use crate::config::BootMode;
            if options.config.boot.mode != BootMode::Menu {
                if let Some(index) = preferred(&state, options) {
                    let environment = &state.environments[index];
                    if !environment.properties.locked()
                        && environment.default_target.is_some()
                        && environment.unavailable.is_none()
                    {
                        let newly_armed = auto_deadline.is_none();
                        let deadline = *auto_deadline.get_or_insert_with(|| {
                            Instant::now()
                                + Duration::from_secs(
                                    if options.config.boot.mode == BootMode::Immediate {
                                        0
                                    } else {
                                        options.config.boot.timeout_secs as u64
                                    },
                                )
                        });
                        state.countdown_secs = Some(
                            deadline
                                .saturating_duration_since(Instant::now())
                                .as_secs()
                                .saturating_add(1) as u32,
                        );
                        if Instant::now() >= deadline && !event::poll(Duration::ZERO)? {
                            state.selected_environment = index;
                            auto_cancelled = true;
                            state.countdown_secs = None;
                            if let Some(action) = intent_action(
                                Intent::Boot,
                                &mut state,
                                &mut ui,
                                options,
                                &mut unlocking,
                            ) {
                                workflow.start(action)?;
                                state.activity = workflow.view();
                            }
                        }
                        if newly_armed || last_tick.elapsed() >= Duration::from_secs(1) {
                            refresh(terminal, &mut state, options, &mut ui)?;
                            emit(
                                options,
                                if newly_armed {
                                    "autoboot-armed"
                                } else {
                                    "autoboot-tick"
                                },
                                &state,
                                &ui,
                            )?;
                            last_tick = Instant::now();
                        }
                    } else if !environment.properties.locked() {
                        auto_cancelled = true;
                        state.error = environment
                            .default_error
                            .clone()
                            .or(environment.unavailable.clone());
                        refresh(terminal, &mut state, options, &mut ui)?;
                    }
                } else {
                    auto_cancelled = true;
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
        events: crate::diagnostics::Events::default(),
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
                options.events = crate::diagnostics::Events::open(
                    std::path::Path::new(&path),
                    options.supervised,
                );
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
