use crate::session::{Client, Request};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    widgets::{Block, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{fs::OpenOptions, io::Write, path::PathBuf, process::Command, time::Duration};
use zbm_core::{State, discover, preview};
mod executor;

struct Options {
    preview: bool,
    supervised: bool,
    events: Option<PathBuf>,
    config: crate::config::Config,
}

fn emit(options: &Options, event: &str, state: &State) -> Result<()> {
    if let Some(path) = &options.events {
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let value = serde_json::json!({"event": event, "state": state, "pid": std::process::id(), "terminal_size": crossterm::terminal::size().ok(), "config": options.config});
        // Construct a complete record before touching the shared serial device.
        // The leading separator also recovers from another writer's partial line.
        let record = format!("\n{value}\n");
        file.write_all(record.as_bytes())?;
    }
    Ok(())
}

fn draw(frame: &mut Frame, state: &State, config: &crate::config::Config) {
    let areas = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(4),
        Constraint::Length(3),
    ])
    .split(frame.area());
    frame.render_widget(
        Paragraph::new(
            config
                .ui
                .title
                .as_deref()
                .unwrap_or("zbm-rs | ZFS boot environment manager"),
        )
        .block(Block::bordered()),
        areas[0],
    );
    let items: Vec<ListItem> = if state.scanning {
        vec![ListItem::new("Discovering pools...")]
    } else if !state.targets.is_empty() {
        state
            .targets
            .iter()
            .map(|target| {
                ListItem::new(format!(
                    "{}  generation {}  {}",
                    target.dataset, target.generation, target.label
                ))
            })
            .collect()
    } else if state.pools.is_empty() {
        vec![ListItem::new("No importable pools discovered")]
    } else {
        state
            .pools
            .iter()
            .map(|p| ListItem::new(format!("{}  {}  GUID {}", p.name, p.health, p.guid)))
            .collect()
    };
    let list = List::new(items)
        .block(Block::bordered().title(if state.targets.is_empty() {
            "ZFS pools"
        } else {
            "NixOS generations"
        }))
        .highlight_style(Style::default().bg(Color::Blue))
        .highlight_symbol("> ");
    let mut selection = ListState::default().with_selected(if !state.targets.is_empty() {
        Some(state.selected_target)
    } else {
        (!state.pools.is_empty()).then_some(state.selected)
    });
    frame.render_stateful_widget(list, areas[1], &mut selection);
    let message = state
        .error
        .as_deref()
        .unwrap_or(if state.targets.is_empty() {"Enter: import the selected pool without force and inspect NixOS generations (read-only mounts)."} else {"Enter: boot the selected generation. R: return to pool discovery."});
    frame.render_widget(
        Paragraph::new(message)
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title("Status")),
        areas[2],
    );
    frame.render_widget(
        Paragraph::new(
            "[Enter] Open / Boot  [R] Rescan  [S] Shell  [N] Restart  [P] Power off  [Q] Exit",
        )
        .block(Block::bordered()),
        areas[3],
    );
}

async fn tui(
    terminal: &mut DefaultTerminal,
    options: &Options,
    channel: Option<&Client>,
) -> Result<()> {
    #[cfg(feature = "vm-test")]
    let control = crate::vm_test::Control::open()?;
    let mut state = State::default();
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mut rescan = true;
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
        if rescan && !state.scanning {
            state.scanning = true;
            rescan = false;
            emit(options, "scanning", &state)?;
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
        let mut ready = false;
        if let Ok(result) = rx.try_recv() {
            state.apply_scan(result);
            ready = true;
        }
        terminal.draw(|f| draw(f, &state, &options.config))?;
        // Publish readiness only after the corresponding frame has been drawn.
        if ready {
            emit(options, "ready", &state)?;
        }
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') => {
                    emit(options, "exit", &state)?;
                    break;
                }
                KeyCode::Char('r') => {
                    rescan = true;
                }
                KeyCode::Up | KeyCode::Down => {
                    if state.targets.is_empty() {
                        state.move_selection(key.code == KeyCode::Down);
                    } else {
                        let length = state.targets.len();
                        state.selected_target = (state.selected_target
                            + if key.code == KeyCode::Down {
                                1
                            } else {
                                length - 1
                            })
                            % length;
                    }
                    emit(options, "selection", &state)?;
                }
                KeyCode::Enter if options.supervised && !options.preview && !state.scanning => {
                    if state.targets.is_empty() {
                        if let Some(pool) = state.pools.get(state.selected) {
                            state.error = Some("Inspecting selected pool...".into());
                            terminal.draw(|frame| draw(frame, &state, &options.config))?;
                            let readonly = matches!(
                                options.config.zfs.import_policy,
                                crate::config::ImportPolicy::ReadOnly
                            );
                            match zbm_core::boot_zfs::targets(
                                &zbm_core::Zfs::new(),
                                pool,
                                readonly,
                                options.config.nixos.generation_limit,
                            )
                            .await
                            {
                                Ok(targets) => {
                                    state.error = targets
                                        .is_empty()
                                        .then(|| "No supported NixOS generations found".into());
                                    state.targets = targets;
                                    state.selected_target = 0;
                                }
                                Err(error) => state.error = Some(error.to_string()),
                            }
                            terminal.draw(|frame| draw(frame, &state, &options.config))?;
                            emit(options, "boot-targets", &state)?;
                        }
                    } else {
                        let preparation = async {
                            let plan = zbm_core::boot::BootPlan::resolve(
                                &state.targets[state.selected_target],
                                &options.config.kernel_args,
                            )?;
                            let loaded = executor::LoadedKernel::load(&plan)?;
                            let pool = &state.pools[state.selected];
                            zbm_core::boot_zfs::release_owned(&zbm_core::Zfs::new(), pool).await?;
                            Ok::<_, anyhow::Error>((plan, loaded))
                        }
                        .await;
                        match preparation {
                            Ok((plan, loaded)) => {
                                let path = "/run/zbm-rs/boot-plan.json";
                                std::fs::write(path, serde_json::to_vec(&plan)?)?;
                                emit(options, "kexec", &state)?;
                                ratatui::restore();
                                channel.unwrap().request(Request::KexecStarting)?;
                                loaded.execute()?;
                            }
                            Err(error) => {
                                state.error = Some(format!("Boot preparation failed: {error:#}"));
                                emit(options, "boot-error", &state)?;
                            }
                        }
                    }
                }
                KeyCode::Char('s') => {
                    emit(options, "shell", &state)?;
                    if let Some(channel) = channel {
                        channel.request(Request::EmergencyShell)?;
                        break;
                    }
                    ratatui::restore();
                    let result = Command::new("/bin/sh").status();
                    *terminal = ratatui::init();
                    result?;
                    emit(options, "shell-return", &state)?;
                }
                KeyCode::Char('n') if channel.is_some() => {
                    if let Some(channel) = channel {
                        channel.request(Request::Restart)?;
                    }
                    break;
                }
                KeyCode::Char('p') if channel.is_some() => {
                    if let Some(channel) = channel {
                        channel.request(Request::PowerOff)?;
                    }
                    break;
                }
                _ => {}
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
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rescan_and_pool_selection_preserve_footer() {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(160, 50)).unwrap();
        let mut state = State::default();
        for phase in 0..3 {
            if phase == 1 {
                state.scanning = true;
            }
            if phase == 2 {
                state.apply_scan(Ok(preview()));
            }
            terminal
                .draw(|frame| draw(frame, &state, &crate::config::Config::default()))
                .unwrap();
            let line: String = (0..160)
                .map(|x| terminal.backend().buffer()[(x, 48)].symbol())
                .collect();
            assert!(
                line.contains("Rescan"),
                "Missing footer in phase {phase}: {line}"
            );
        }
    }
}
