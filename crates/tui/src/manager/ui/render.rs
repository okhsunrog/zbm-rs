//! Console-first presentation: ANSI colors, readable selection and concise summaries.
use super::*;
use crate::manager::status_message;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::Modifier,
    widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap},
};

// Use the base ANSI backgrounds: bright backgrounds are not reliable on Linux VT.
const BASE: Color = Color::Black;
const INK: Color = Color::White;
const MUTED: Color = Color::Gray;
const CHROME: Color = Color::Blue;
const FOCUS: Color = Color::Cyan;
const SNAPSHOT: Color = Color::Magenta;
const GOOD: Color = Color::Green;
const NOTICE: Color = Color::Yellow;
const DANGER: Color = Color::Red;

fn ink(color: Color) -> Style {
    Style::default().fg(color)
}
fn strong(color: Color) -> Style {
    ink(color).add_modifier(Modifier::BOLD)
}
fn heading(text: impl Into<String>, color: Color) -> Line<'static> {
    Line::styled(text.into(), strong(color))
}
fn field(label: &str, value: impl AsRef<str>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<12}"), ink(MUTED)),
        Span::styled(clean(value), ink(INK)),
    ])
}
fn keycap(key: &str, label: &str, color: Color) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            format!(" {key} "),
            ink(if color == CHROME { INK } else { BASE }).bg(color),
        ),
        Span::styled(format!(" {label}  "), ink(INK)),
    ]
}
fn panel(title: impl Into<String>, color: Color) -> Block<'static> {
    Block::bordered()
        .border_style(ink(color))
        .title(Span::styled(format!(" {} ", title.into()), strong(color)))
        .padding(Padding::horizontal(1))
        .style(ink(INK).bg(BASE))
}
fn tag_style(tag: &str) -> Style {
    ink(if tag.contains("UNAVAILABLE") {
        NOTICE
    } else if tag == "SNAPSHOT" {
        SNAPSHOT
    } else if tag.contains("BOOTFS") {
        GOOD
    } else {
        FOCUS
    })
}

impl Ui {
    fn summary(&self, state: &State, options: &Options) -> Vec<Line<'static>> {
        let mut lines = vec![Line::raw("")];
        match self.selected(state).map(|row| row.kind) {
            Some(Kind::Generation(i)) => {
                let target = &state.targets[i];
                lines.push(heading(clean(&target.label), INK));
                lines.push(Line::raw(""));
                if let Some(generation) = target.generation {
                    lines.push(field("Generation", generation.to_string()));
                }
                lines.push(field("Dataset", &target.dataset));
                lines.push(field(
                    "Source",
                    if target.snapshot.is_some() {
                        "ZFS snapshot"
                    } else {
                        "Live boot environment"
                    },
                ));
                if let Some(snapshot) = &target.snapshot {
                    lines.push(Line::raw(""));
                    lines.push(heading("Snapshot", SNAPSHOT));
                    lines.push(Line::styled(clean(&snapshot.name), ink(INK)));
                    lines.push(Line::raw(""));
                    if self.prepared_target(target).is_some() {
                        lines.push(heading("Clone prepared", GOOD));
                        lines.push(Line::raw("Enter boots the writable clone."));
                        lines.push(Line::raw("The source snapshot stays unchanged."));
                    } else {
                        lines.push(heading("Boot with a writable clone", SNAPSHOT));
                        lines.push(Line::raw(
                            "Enter prepares or reuses a clone, then boots it.",
                        ));
                        lines.push(Line::raw("C prepares the clone without booting."));
                    }
                }
                if let Some(issue) = &target.issue {
                    lines.push(Line::raw(""));
                    lines.push(heading("Unavailable", NOTICE));
                    lines.push(Line::raw(clean(issue)));
                }
                lines.push(Line::raw(""));
                lines.push(heading("Boot checks", FOCUS));
                lines.push(Line::raw(
                    if options.config.security.mode == crate::config::SecurityMode::Enforce {
                        "Owner authorization is checked before loading."
                    } else {
                        "Boot inputs are rechecked before loading."
                    },
                ));
            }
            Some(Kind::Pool(i)) => {
                let pool = &state.pools[i];
                lines.push(heading(clean(&pool.name), INK));
                lines.push(Line::raw(""));
                lines.push(field("Health", &pool.health));
                lines.push(field(
                    "Import",
                    match options.config.zfs.import_policy {
                        crate::config::ImportPolicy::ReadOnly => "Read-only",
                        crate::config::ImportPolicy::HostId => "Host ID guarded",
                    },
                ));
                lines.push(Line::raw(""));
                lines.push(heading("Start here", FOCUS));
                lines.push(Line::raw(
                    "Enter imports this pool and finds boot environments.",
                ));
                lines.push(Line::raw("Pools are never force-imported automatically."));
            }
            Some(Kind::Environment(i)) => {
                let env = &state.environments[i];
                lines.push(heading(clean(&env.dataset), INK));
                lines.push(Line::raw(""));
                if env.is_default {
                    lines.push(heading("Default boot environment [BOOTFS]", GOOD));
                }
                lines.push(field("Boot targets", env.targets.len().to_string()));
                lines.push(field(
                    "Unavailable",
                    env.rejected_generations.len().to_string(),
                ));
                lines.push(field("Encryption", &env.properties.encryption));
                lines.push(Line::raw(""));
                if let Some(error) = &env.unavailable {
                    lines.push(heading("Unavailable", NOTICE));
                    lines.push(Line::raw(clean(error)));
                } else {
                    lines.push(heading("Choose a boot target", FOCUS));
                    lines.push(Line::raw(
                        "Enter opens the available generations or kernels.",
                    ));
                    if options.config.ui.show_snapshots {
                        lines.push(Line::raw("T opens this environment's snapshots."));
                    }
                }
            }
            Some(Kind::Snapshot(i)) => {
                let snapshot = &state.snapshots[i];
                lines.push(heading(clean(&snapshot.source.name), SNAPSHOT));
                lines.push(Line::raw(""));
                lines.push(heading("Inspect before booting", INK));
                lines.push(Line::raw(
                    "Enter opens the boot targets saved in this snapshot.",
                ));
                lines.push(Line::raw("Booting a snapshot uses a writable clone."));
                lines.push(Line::raw(""));
                lines.push(heading("Snapshot management", SNAPSHOT));
                lines.push(Line::raw("F2 lists clone, promotion and rollback actions."));
                lines.push(Line::raw("Availability depends on the image policy."));
            }
            Some(Kind::Rejected(i)) => {
                let issue = &state.rejected_generations[i];
                lines.push(heading(format!("Generation {}", issue.generation), INK));
                lines.push(Line::raw(""));
                lines.push(heading("Unavailable for boot", NOTICE));
                lines.push(Line::raw(clean(&issue.error)));
                lines.push(Line::raw(""));
                lines.push(Line::raw("Select another generation to boot."));
            }
            None => {
                lines.push(heading(
                    if state.scanning {
                        "Discovering ZFS pools"
                    } else {
                        "Nothing selected"
                    },
                    FOCUS,
                ));
                lines.push(Line::raw(""));
                lines.push(Line::raw(if !self.screens[&self.key].query.is_empty() {
                    "Esc clears the filter. / starts a new search."
                } else {
                    "R rescans for pools. F2 lists recovery actions."
                }));
            }
        }
        lines
    }

    fn help(&self, protected: bool) -> Vec<Line<'static>> {
        let mut lines = vec![];
        for (title, color, commands) in [
            (
                "Navigation & boot",
                FOCUS,
                &[
                    Command::Open,
                    Command::Back,
                    Command::Search,
                    Command::Details,
                    Command::Help,
                    Command::Actions,
                ][..],
            ),
            (
                "Snapshots",
                SNAPSHOT,
                &[
                    Command::Snapshots,
                    Command::Prepare,
                    Command::Discard,
                    Command::Clone,
                    Command::ClonePromote,
                    Command::Rollback,
                ][..],
            ),
            (
                "Recovery & power",
                NOTICE,
                &[
                    Command::Rescan,
                    Command::Shell,
                    Command::Restart,
                    Command::PowerOff,
                    Command::Exit,
                ][..],
            ),
        ] {
            lines.push(heading(title, color));
            for &command in commands {
                let disabled = protected
                    && matches!(
                        command,
                        Command::Shell | Command::Rollback | Command::Clone | Command::ClonePromote
                    );
                lines.push(Line::from(vec![
                    Span::styled(format!("  {:12}", command.keys()), strong(color)),
                    Span::styled(command.label(), ink(INK)),
                    Span::styled(
                        if disabled {
                            " (disabled in enforce)"
                        } else {
                            ""
                        },
                        ink(NOTICE),
                    ),
                ]));
            }
            lines.push(Line::raw(""));
        }
        lines.extend([
            Line::raw("Arrow / Home / End / PgUp / PgDn: select / scroll"),
            Line::raw("Search: Enter keeps filter; Esc clears it. Typing never runs commands."),
            Line::raw(if protected {
                "F6 Restart manager and F10 Power off work during search and operations."
            } else {
                "F4 Shell, F6 Restart manager, F10 Power off work during search and operations."
            }),
            Line::raw("Q exits the manager; PID 1 decides recovery. N restarts only the manager."),
        ]);
        lines
    }

    pub fn draw(&mut self, frame: &mut Frame, state: &State, options: &Options) {
        let area = frame.area();
        frame.render_widget(Block::default().style(ink(INK).bg(BASE)), area);
        // A boot picker should remain a compact composition on large consoles.
        // Small consoles keep every cell; longer lists scroll within this viewport.
        let width = area.width.min(144);
        let height = area.height.min(38);
        let area = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let protected = options.config.security.mode == crate::config::SecurityMode::Enforce;
        let wide = area.width >= 110;
        let [header, context_area, body, status, primary, recovery] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Length(if area.width < 76 { 3 } else { 2 }),
        ])
        .areas(area);
        frame.render_widget(Block::default().style(ink(INK).bg(CHROME)), header);
        let title = options.config.ui.title.as_deref().unwrap_or("zbm-rs");
        let [brand, policy] = Layout::horizontal([
            Constraint::Min(1),
            Constraint::Length(if wide { 37 } else { 22 }),
        ])
        .areas(header.inner(Margin::new(2, 1)));
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!(" {} ", clean(title)),
                    ink(BASE).bg(FOCUS).add_modifier(Modifier::BOLD),
                ),
                Span::styled(if wide { "   ZFS boot manager" } else { "" }, strong(INK)),
            ])),
            brand,
        );
        frame.render_widget(
            Paragraph::new(if protected {
                "Boot policy: enforce"
            } else {
                "Boot policy: off"
            })
            .style(ink(if protected { INK } else { NOTICE }))
            .right_aligned(),
            policy,
        );
        let context = if state.snapshot_view {
            state
                .snapshots
                .get(state.selected_snapshot)
                .map(|s| s.source.name.as_str())
                .or_else(|| {
                    state
                        .environments
                        .get(state.selected_environment)
                        .map(|e| e.dataset.as_str())
                })
        } else {
            state
                .environments
                .get(state.selected_environment)
                .map(|e| e.dataset.as_str())
                .or_else(|| state.pools.get(state.selected).map(|p| p.name.as_str()))
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("  ZFS  /  ", ink(FOCUS)),
                Span::styled(
                    clean(context.unwrap_or("Discover pools")),
                    ink(if state.snapshot_view { SNAPSHOT } else { INK }),
                ),
            ])),
            context_area,
        );
        let body = body.inner(Margin::new(2, 0));
        let columns = if wide {
            Layout::horizontal([
                Constraint::Percentage(52),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .split(body)
            .to_vec()
        } else {
            vec![body]
        };
        let rows = self.visible(state);
        let list_block = panel(
            format!("{}  ({})", View::of(state).title(), rows.len()),
            FOCUS,
        );
        let inner = list_block.inner(columns[0]);
        frame.render_widget(list_block, columns[0]);
        let [filter, list] =
            Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(inner);
        let query = &self.screens[&self.key].query;
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    if self.searching { " / Find: " } else { " / " },
                    strong(FOCUS),
                ),
                Span::styled(
                    if self.searching {
                        format!("{}_", clean(query))
                    } else if query.is_empty() {
                        "Find in this list".into()
                    } else {
                        format!("{}   [Esc clears]", clean(query))
                    },
                    ink(if self.searching { INK } else { MUTED }),
                ),
            ])),
            filter,
        );
        let items = if state.scanning {
            vec![ListItem::new("Discovering pools...")]
        } else if rows.is_empty() {
            vec![ListItem::new(vec![
                Line::raw(if query.is_empty() {
                    "No candidates discovered"
                } else {
                    "No targets match this filter"
                }),
                Line::raw(""),
                Line::styled(
                    if query.is_empty() {
                        "R Rescan    F2 Recovery actions"
                    } else {
                        "Esc Clear filter    / Search"
                    },
                    ink(FOCUS),
                ),
            ])]
        } else {
            rows.iter()
                .map(|row| {
                    ListItem::new(vec![
                        Line::from(vec![
                            Span::styled(clean(&row.title), strong(INK)),
                            Span::styled(format!("  [{}]", row.tag), tag_style(row.tag)),
                        ]),
                        Line::styled(format!("  {}", clean(&row.subtitle)), ink(MUTED)),
                        Line::raw(""),
                    ])
                })
                .collect()
        };
        frame.render_stateful_widget(
            List::new(items)
                .highlight_symbol("> ")
                .highlight_style(ink(BASE).bg(FOCUS).add_modifier(Modifier::BOLD)),
            list,
            &mut self.screens.get_mut(&self.key).unwrap().list,
        );
        if wide {
            let block = panel(
                "Selected target",
                if state.snapshot_view {
                    SNAPSHOT
                } else {
                    Color::LightBlue
                },
            );
            let inner = block.inner(columns[2]);
            frame.render_widget(block, columns[2]);
            let [summary, hint] =
                Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(inner);
            frame.render_widget(
                Paragraph::new(self.summary(state, options)).wrap(Wrap { trim: false }),
                summary,
            );
            frame.render_widget(
                Paragraph::new(Line::from(keycap("I", "Full paths & boot details", FOCUS))),
                hint,
            );
        }
        let mut message = if state.error.is_none()
            && state.operation.is_none()
            && self
                .target(state)
                .is_some_and(|t| self.prepared_target(t).is_some())
        {
            "Clone prepared. Enter boots it; I shows the full boot plan.".into()
        } else {
            status_message(state)
        };
        if let Some(started) = self.operation_started.filter(|_| state.operation.is_some()) {
            message.push_str(&format!(
                "  [{}s / 30s deadline]",
                started.elapsed().as_secs()
            ));
        }
        let (label, color) = if state.requires_restart {
            (" Recovery required ", DANGER)
        } else if state.error.is_some() {
            (" Notice ", NOTICE)
        } else if state.operation.is_some() || state.scanning {
            (" Working ", FOCUS)
        } else {
            (" > ", FOCUS)
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(label, strong(color)),
                Span::styled(
                    clean(message),
                    ink(if state.error.is_some() { NOTICE } else { MUTED }),
                ),
            ]))
            .wrap(Wrap { trim: false }),
            status.inner(Margin::new(2, 0)),
        );
        let mut actions = keycap("Enter", self.open_label(state), FOCUS);
        actions.extend(keycap("Esc", "Back", MUTED));
        if !state.environments.is_empty() && options.config.ui.show_snapshots {
            actions.extend(keycap("T", "Snapshots", SNAPSHOT));
        }
        actions.extend(keycap("I", "Details", FOCUS));
        actions.extend(keycap("F2", "Actions", FOCUS));
        actions.extend(keycap("F1", "Help", MUTED));
        frame.render_widget(
            Paragraph::new(Line::from(actions)).wrap(Wrap { trim: false }),
            primary.inner(Margin::new(2, 0)),
        );
        let mut actions = keycap("F5", "Rescan", CHROME);
        if !protected {
            actions.extend(keycap("F4", "Shell", CHROME));
        }
        actions.extend(keycap("F6", "Restart", CHROME));
        actions.extend(keycap("F10", "Power off", CHROME));
        frame.render_widget(
            Paragraph::new(Line::from(actions)).wrap(Wrap { trim: false }),
            recovery.inner(Margin::new(2, 0)),
        );
        if self.panel != Panel::None {
            self.draw_panel(frame, state, options, protected);
        }
    }
    fn draw_panel(&mut self, frame: &mut Frame, state: &State, options: &Options, protected: bool) {
        let area = frame.area();
        let width = area.width.saturating_sub(4).min(106);
        let height = area.height.saturating_sub(2).min(match self.panel {
            Panel::Help => 32,
            Panel::Actions => 24,
            Panel::Details => 34,
            _ => 14,
        });
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let shadow = Rect::new(
            popup.x.saturating_add(1),
            popup.y.saturating_add(1),
            popup.width,
            popup.height,
        )
        .intersection(area);
        frame.render_widget(Block::default().style(ink(BASE).bg(CHROME)), shadow);
        frame.render_widget(Clear, popup);
        let accent = match self.panel {
            Panel::ConfirmRollback | Panel::ConfirmDiscard => DANGER,
            Panel::ConfirmPromote => NOTICE,
            _ => FOCUS,
        };
        let mut block = panel(
            match self.panel {
                Panel::Help => "Keyboard help [Up/Down] scroll [Esc] close",
                Panel::Actions => "All actions [Enter] run [Esc] close",
                Panel::Details => "Full target details [PgUp/PgDn] scroll [Esc] close",
                Panel::ConfirmDiscard => "Discard prepared clone? [Enter] confirm [Esc] cancel",
                Panel::ConfirmRollback => "Rollback dataset [Esc] cancel",
                Panel::ConfirmPromote => "Clone and promote [Enter] confirm [Esc] cancel",
                Panel::None => unreachable!(),
            },
            accent,
        );
        if self.panel == Panel::Actions {
            if popup.height < 19 {
                block = block.borders(Borders::TOP);
            }
            let [menu_area, reason_area] = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(if popup.height >= 23 { 3 } else { 0 }),
            ])
            .areas(block.inner(popup));
            frame.render_widget(block, popup);
            let items: Vec<_> = ALL
                .into_iter()
                .map(|command| {
                    let disabled = self.reason(command, state, options).is_some();
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("{:12}", command.keys()),
                            ink(if disabled { MUTED } else { FOCUS }),
                        ),
                        Span::styled(command.label(), ink(if disabled { MUTED } else { INK })),
                        Span::styled(if disabled { "  [unavailable]" } else { "" }, ink(NOTICE)),
                    ]))
                })
                .collect();
            frame.render_stateful_widget(
                List::new(items)
                    .highlight_symbol("> ")
                    .highlight_style(ink(BASE).bg(FOCUS).add_modifier(Modifier::BOLD)),
                menu_area,
                &mut self.menu,
            );
            let command = ALL[self.menu.selected().unwrap_or(0)];
            let explanation = self
                .reason(command, state, options)
                .map(|reason| format!("Unavailable: {}", clean(reason)))
                .unwrap_or_else(|| format!("Enter: {}", command.label()));
            frame.render_widget(
                Paragraph::new(explanation)
                    .style(ink(NOTICE))
                    .block(
                        Block::default()
                            .borders(Borders::TOP)
                            .border_style(ink(CHROME)),
                    )
                    .wrap(Wrap { trim: false }),
                reason_area,
            );
        } else {
            let text = match self.panel {
                Panel::Help => self.help(protected),
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
                    Line::raw("Discard current changes and ALL newer snapshots of this dataset."),
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
                    Line::raw("The backend verifies ownership and uses non-recursive destruction."),
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

#[cfg(test)]
mod tests {
    use super::*;
    use zbm_core::boot::{Backend, BootInputs, SnapshotSource};

    #[test]
    fn clone_summary_tracks_the_selected_generation_and_keeps_paths_in_details() {
        let target = BootTarget {
            dataset: "tank/nixos".into(),
            generation: Some(2),
            label: "NixOS (Linux 6.18)".into(),
            root: "/run/snapshot".into(),
            toplevel: Some("/nix/store/system".into()),
            backend: Backend::Nixos,
            snapshot: Some(SnapshotSource {
                name: "tank/nixos@good".into(),
                guid: 42,
            }),
            mountpoint: None,
            issue: None,
            inputs: BootInputs {
                kernel: "/nix/store/kernel/bzImage".into(),
                initrd: Some("/nix/store/initrd/initrd".into()),
                init: Some("/nix/store/system/init".into()),
                kernel_params: vec![],
            },
        };
        let mut older = target.clone();
        older.generation = Some(1);
        let mut clone = target.clone();
        clone.dataset = "tank/zbm-rs-owned-clone".into();
        let mut state = State {
            targets: vec![target, older],
            snapshot_view: true,
            ..State::default()
        };
        let options = Options {
            config: crate::config::Config::default(),
            preview: false,
            supervised: true,
            events: None,
        };
        let mut ui = Ui::default();
        ui.sync(&mut state);
        ui.prepared = Some(BootPlan {
            target: clone,
            kernel: "/run/snapshot/kernel".into(),
            initrd: None,
            cmdline: vec!["root=tank/zbm-rs-owned-clone".into()],
        });
        let text = |lines: Vec<Line<'static>>| {
            lines
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        };
        let summary = text(ui.summary(&state, &options));
        assert!(summary.contains("Clone prepared") && summary.contains("tank/nixos@good"));
        assert!(!summary.contains("/nix/store/"));
        let details = text(ui.details(&state, &options));
        assert!(
            details.contains("/nix/store/kernel/bzImage")
                && details.contains("root=tank/zbm-rs-owned-clone")
        );
        ui.input(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
        let summary = text(ui.summary(&state, &options));
        assert!(!summary.contains("Clone prepared"));
        assert!(summary.contains("prepares or reuses a clone"));
    }
}
