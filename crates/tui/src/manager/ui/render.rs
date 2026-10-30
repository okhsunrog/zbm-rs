//! Console-first presentation: ANSI colors, readable selection and concise summaries.
use super::*;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::Modifier,
    widgets::{Block, Clear, List, ListItem, Padding, Paragraph, Wrap},
};

// Use the base ANSI backgrounds: bright backgrounds are not reliable on Linux VT.
const BASE: Color = Color::Black;
const INK: Color = Color::White;
const MUTED: Color = Color::Gray;
const CHROME: Color = Color::DarkGray;
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
        Span::styled(key.to_owned(), strong(color)),
        Span::styled(format!(" {label}  "), ink(MUTED)),
    ]
}

fn panel(title: impl Into<String>, color: Color) -> Block<'static> {
    Block::bordered()
        .border_style(ink(CHROME))
        .title(Span::styled(format!(" {} ", title.into()), strong(color)))
        .padding(Padding::horizontal(1))
        .style(ink(INK).bg(BASE))
}
fn tag_style(tag: &str) -> Style {
    ink(
        if tag.contains("UNAVAILABLE") || tag.contains("LOCKED") || tag.contains("ERROR") {
            NOTICE
        } else if tag == "SNAPSHOT" {
            SNAPSHOT
        } else if tag.contains("BOOTFS") {
            GOOD
        } else {
            FOCUS
        },
    )
}

impl Ui {
    fn summary(&self, state: &State, options: &Options) -> Vec<Line<'static>> {
        let mut lines = vec![Line::raw("")];
        if self.resume_selected(state) {
            lines.push(heading("Resume saved session", GOOD));
            lines.push(Line::raw("Enter continues where you left off."));
            lines.push(Line::raw("F2 offers session discard and cold boot."));
            lines.push(Line::raw(""));
        }
        match self.selected(state).map(|row| row.kind) {
            Some(Kind::Generation(i)) => {
                let target = &state.targets[i];
                lines.push(heading(clean(&target.label), INK));
                lines.push(Line::raw(""));
                if let Some(generation) = target.generation() {
                    lines.push(field("Generation", generation.to_string()));
                    if let Some(name) = target.specialisation() {
                        lines.push(field("Variant", clean(name)));
                    }
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
                if let Some(issue) = &target.issue() {
                    lines.push(Line::raw(""));
                    lines.push(heading("Unavailable", NOTICE));
                    lines.push(Line::raw(clean(issue)));
                }
            }
            Some(Kind::Pool(i)) => {
                let pool = &state.pools[i];
                lines.push(heading(clean(&pool.name), INK));
                lines.push(Line::raw(""));
                lines.push(field("Health", &pool.health));
                lines.push(field(
                    "Import",
                    match options.config.zfs.import_policy {
                        _ if state.resume.protected() => "Read-only while a saved session exists",
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
                    "Rejected",
                    env.rejected_generations.len().to_string(),
                ));
                lines.push(field("Encryption", &env.properties.encryption));
                lines.push(Line::raw(""));
                if let Some(error) = &env.unavailable {
                    lines.push(heading(
                        if env.properties.locked() {
                            "Locked"
                        } else {
                            "Boot unavailable"
                        },
                        NOTICE,
                    ));
                    lines.push(Line::raw(clean(error)));
                } else {
                    lines.push(heading("Boot this system", FOCUS));
                    lines.push(Line::raw(if self.resume_selected(state) {
                        "Enter resumes the saved session. K opens other targets."
                    } else {
                        "Enter boots the default. K opens kernels or generations."
                    }));
                    if options.config.ui.show_snapshots {
                        lines.push(Line::raw("T opens this environment's snapshots."));
                    }
                }
            }
            Some(Kind::Snapshot(i)) => {
                let snapshot = &state.snapshots[i];
                lines.push(heading(clean(&snapshot.source.name), SNAPSHOT));
                lines.push(Line::raw(""));
                lines.push(field("Created", snapshot_date(snapshot.creation)));
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
                    if state.activity.is_scanning() {
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
            Line::raw("Help/details accept only scrolling, Esc and F4/F6/F10 recovery."),
            Line::raw("Q exits the manager; N restarts only the manager, not the machine."),
        ]);
        lines
    }

    fn action_label(&self, command: Command, state: &State) -> String {
        if command == Command::Open && self.resume_selected(state) {
            return "Resume saved session".into();
        }
        if command != Command::Open {
            return command.label().into();
        }
        match self.selected(state).map(|row| row.kind) {
            Some(Kind::Pool(i)) => format!("Import pool {}", clean(&state.pools[i].name)),
            Some(Kind::Environment(i)) => format!(
                "{} {}",
                self.open_label(state),
                clean(&state.environments[i].dataset)
            ),
            Some(Kind::Snapshot(_)) => "Inspect snapshot boot targets".into(),
            Some(Kind::Generation(i)) => {
                let target = &state.targets[i];
                let name = target
                    .generation()
                    .map(|g| variant_title(g, target.specialisation()))
                    .unwrap_or_else(|| clean(&target.label));
                format!(
                    "Boot {name}{}",
                    if target.snapshot.is_some() {
                        " from clone"
                    } else {
                        ""
                    }
                )
            }
            Some(Kind::Rejected(i)) => format!(
                "Boot generation {}",
                state.rejected_generations[i].generation
            ),
            None => "Open selection".into(),
        }
    }

    fn selection_hint(&self, state: &State, options: &Options) -> String {
        if let Some(reason) = self.reason(Command::Open, state, options) {
            return format!("Unavailable: {}", clean(reason));
        }
        if self.resume_selected(state) {
            return "Enter resumes the saved session. F2 offers discard and cold boot.".into();
        }
        if self.cancels_resume(Command::Open, state) {
            return "Booting this system requires discarding the saved session. Confirmation will be requested.".into();
        }
        match self.selected(state).map(|row| row.kind) {
            Some(Kind::Generation(i)) if state.targets[i].snapshot.is_some() => {
                if self.prepared_target(&state.targets[i]).is_some() {
                    "Clone prepared. Boot uses the writable clone; the snapshot stays unchanged."
                        .into()
                } else {
                    "Boot creates or reuses a writable clone; the snapshot stays unchanged.".into()
                }
            }
            Some(Kind::Generation(_)) => "Ready to boot the selected system.".into(),
            Some(Kind::Pool(_)) => {
                "Import this pool to discover boot environments. No force import.".into()
            }
            Some(Kind::Environment(i)) if state.environments[i].properties.locked() => {
                "Unlock the required encryption root, then continue the selected action.".into()
            }
            Some(Kind::Environment(_)) => {
                "Enter boots the default. K chooses a kernel or generation.".into()
            }
            Some(Kind::Snapshot(_)) => {
                "Inspect saved boot targets. F2 offers clone and rollback actions.".into()
            }
            _ => "Select an entry to continue.".into(),
        }
    }

    fn primary_keys(&self, state: &State, options: &Options, compact: bool) -> Line<'static> {
        let mut keys = Vec::new();
        if self.panel != Panel::None {
            if matches!(self.panel, Panel::Help | Panel::Details) {
                keys.extend(keycap("Up/Down", "Scroll", MUTED));
                keys.extend(keycap("Esc", "Close", FOCUS));
            } else if self.panel == Panel::Actions {
                keys.extend(keycap("Up/Down", "Select", MUTED));
                keys.extend(keycap("Enter", "Run action", FOCUS));
                keys.extend(keycap("Esc", "Close", MUTED));
            } else {
                keys.extend(keycap(
                    "Enter",
                    if self.panel == Panel::Unlock {
                        "Unlock"
                    } else {
                        "Confirm"
                    },
                    FOCUS,
                ));
                keys.extend(keycap("Esc", "Cancel", MUTED));
            }
            return Line::from(keys);
        }
        if self.searching {
            keys.extend(keycap("Enter", "Keep filter", FOCUS));
            keys.extend(keycap("Esc", "Clear", MUTED));
        } else {
            let disabled = self.reason(Command::Open, state, options).is_some();
            keys.extend(keycap(
                "Enter",
                if disabled {
                    "Unavailable"
                } else {
                    self.open_label(state)
                },
                if disabled { CHROME } else { FOCUS },
            ));
            keys.extend(keycap(
                "Esc",
                if self.screens[&self.key].query.is_empty() {
                    "Back"
                } else {
                    "Clear filter"
                },
                MUTED,
            ));
            if !compact && !state.environments.is_empty() && options.config.ui.show_snapshots {
                keys.extend(keycap("T", "Snapshots", MUTED));
            }
            if !state.environments.is_empty() {
                keys.extend(keycap("K", "Kernels", MUTED));
            }
            keys.extend(keycap("I", "Details", MUTED));
            keys.extend(keycap("F2", "Actions", MUTED));
        }
        keys.extend(keycap("F1", "Help", MUTED));
        Line::from(keys)
    }

    pub fn draw(&mut self, frame: &mut Frame, state: &State, options: &Options) {
        let screen = frame.area();
        frame.render_widget(Block::default().style(ink(INK).bg(BASE)), screen);
        let width = screen.width.min(144);
        let height = screen.height.min(38);
        let area = Rect::new(
            screen.x + (screen.width - width) / 2,
            screen.y + (screen.height - height) / 2,
            width,
            height,
        );
        let compact = area.width < 110 || area.height < 30;
        let protected = options.config.security.mode == crate::config::SecurityMode::Enforce;
        let margin = if compact { 1 } else { 2 };
        let primary_text =
            Paragraph::new(self.primary_keys(state, options, compact)).wrap(Wrap { trim: false });
        let mut recovery_keys = if self.panel == Panel::None {
            keycap("F5", "Rescan", MUTED)
        } else {
            vec![]
        };
        if !protected {
            recovery_keys.extend(keycap("F4", "Shell", MUTED));
        }
        recovery_keys.extend(keycap("F6", "Restart", MUTED));
        recovery_keys.extend(keycap("F10", "Power off", MUTED));
        let recovery_text = Paragraph::new(Line::from(recovery_keys)).wrap(Wrap { trim: false });
        let content_width = area.width.saturating_sub(margin * 2);
        let [header, context_area, body, status, primary, recovery] = Layout::vertical([
            Constraint::Length(if compact { 1 } else { 2 }),
            Constraint::Length(if compact { 1 } else { 2 }),
            Constraint::Min(3),
            Constraint::Length(if compact { 2 } else { 3 }),
            Constraint::Length(primary_text.line_count(content_width) as u16),
            Constraint::Length(recovery_text.line_count(content_width) as u16),
        ])
        .areas(area);
        let [brand, policy] = Layout::horizontal([Constraint::Min(1), Constraint::Length(22)])
            .areas(header.inner(Margin::new(margin, 0)));
        let title = options.config.ui.title.as_deref().unwrap_or("zbm-rs");
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(clean(title), strong(INK)),
                Span::styled(
                    if compact {
                        ""
                    } else {
                        "   /   ZFS boot manager"
                    },
                    ink(MUTED),
                ),
            ])),
            brand,
        );
        frame.render_widget(
            Paragraph::new(if protected {
                "Boot policy: enforce"
            } else {
                "Boot policy: off"
            })
            .style(ink(MUTED))
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
                Span::styled(
                    match &state.resume {
                        zbm_core::resume::Status::Inactive => "",
                        zbm_core::resume::Status::Locked { .. } => "[Swap locked] ",
                        zbm_core::resume::Status::Pending { .. } => "[Resume pending] ",
                        zbm_core::resume::Status::Blocked { .. }
                        | zbm_core::resume::Status::Unavailable { .. } => "[Resume unavailable] ",
                    },
                    strong(NOTICE),
                ),
                Span::styled("ZFS / ", ink(MUTED)),
                Span::styled(
                    clean(context.unwrap_or("Discover pools")),
                    ink(if state.snapshot_view { SNAPSHOT } else { MUTED }),
                ),
            ])),
            context_area.inner(Margin::new(margin, 0)),
        );
        let body = body.inner(Margin::new(margin, 0));
        let columns = if compact {
            vec![body]
        } else {
            Layout::horizontal([
                Constraint::Percentage(55),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .split(body)
            .to_vec()
        };
        let rows = self.visible(state);
        let position = self.screens[&self.key].list.selected().map_or(0, |i| i + 1);
        let query = &self.screens[&self.key].query;
        let count = if query.is_empty() {
            format!(" {position}/{} ", rows.len())
        } else {
            format!(
                " {position}/{} matches ({} total) ",
                rows.len(),
                Self::rows(state).len()
            )
        };
        let list_block = panel(View::of(state).title(), INK)
            .title_bottom(Line::styled(count, ink(MUTED)).right_aligned());
        let inner = list_block.inner(columns[0]);
        frame.render_widget(list_block, columns[0]);
        let [filter, list] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    if self.searching { "/ Find: " } else { "/ " },
                    ink(if self.searching { FOCUS } else { MUTED }),
                ),
                Span::styled(
                    if self.searching {
                        format!("{}_", clean(query))
                    } else if query.is_empty() {
                        "Find in this list".into()
                    } else {
                        format!("{}  [Esc clears]", clean(query))
                    },
                    ink(MUTED),
                ),
            ])),
            filter,
        );
        self.page_size = (list.height as usize / 2).max(1);
        let items = if state.activity.is_scanning() {
            vec![ListItem::new("Discovering pools...")]
        } else if rows.is_empty() {
            vec![ListItem::new(vec![
                Line::raw(if query.is_empty() {
                    "No candidates discovered"
                } else {
                    "No targets match this filter"
                }),
                Line::styled(
                    if query.is_empty() {
                        "F5 Rescan    F2 Actions"
                    } else {
                        "Esc Clear filter"
                    },
                    ink(MUTED),
                ),
            ])]
        } else {
            rows.iter()
                .map(|row| {
                    // Only show tags that distinguish state; ordinary backend names add noise.
                    let resumable = match row.kind {
                        Kind::Generation(i) => state
                            .pools
                            .get(state.selected)
                            .is_some_and(|p| state.resume.matches(p.guid, &state.targets[i])),
                        Kind::Environment(i) => state.resume.session().is_some_and(|s| {
                            s.pool_guid == state.environments[i].pool_guid
                                && s.target.dataset == state.environments[i].dataset
                        }),
                        _ => false,
                    };
                    let mut tag = if matches!(row.tag, "NIXOS" | "KERNEL" | "POOL" | "BE") {
                        String::new()
                    } else {
                        format!("  [{}]", row.tag)
                    };
                    if resumable {
                        tag.push_str("  [RESUME]");
                    }
                    ListItem::new(vec![
                        Line::from(vec![
                            Span::styled(clean(&row.title), strong(INK)),
                            Span::styled(tag, tag_style(row.tag)),
                        ]),
                        Line::styled(format!("  {}", clean(&row.subtitle)), ink(MUTED)),
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
        if !compact {
            let block = panel("Selection", INK);
            let inner = block.inner(columns[2]);
            frame.render_widget(block, columns[2]);
            frame.render_widget(
                Paragraph::new(self.summary(state, options)).wrap(Wrap { trim: false }),
                inner,
            );
        }
        let (label, color, mut message) = if state.activity.requires_restart() {
            (
                "Recovery required: ",
                DANGER,
                state
                    .error
                    .clone()
                    .unwrap_or_else(|| "Restart manager to reconcile the operation.".into()),
            )
        } else if let Some(error) = &state.error {
            ("Notice: ", NOTICE, format!("{}  [I Details]", clean(error)))
        } else if let Some(operation) = state.activity.operation() {
            ("Working: ", FOCUS, operation.to_owned())
        } else if state.activity.is_scanning() {
            ("Working: ", FOCUS, "Discovering ZFS pools...".into())
        } else if let Some(seconds) = state.countdown_secs {
            (
                "Autoboot: ",
                FOCUS,
                if state.resume.session().is_some() {
                    format!("Resuming saved session in {seconds}s. Any key opens the menu.")
                } else {
                    format!("Booting the default system in {seconds}s. Any key opens the menu.")
                },
            )
        } else if self.searching {
            (
                "",
                MUTED,
                "Type to filter. Enter keeps the filter; Esc clears it.".into(),
            )
        } else {
            ("", MUTED, self.selection_hint(state, options))
        };
        if let Some(started) = self
            .operation_started
            .filter(|_| state.activity.operation().is_some())
        {
            message.push_str(&format!(
                "  [{}s / 30s deadline]",
                started.elapsed().as_secs()
            ));
        }
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(label, strong(color)),
                Span::styled(clean(message), ink(color)),
            ]))
            .wrap(Wrap { trim: false }),
            status.inner(Margin::new(margin, 0)),
        );
        frame.render_widget(primary_text, primary.inner(Margin::new(margin, 0)));
        frame.render_widget(recovery_text, recovery.inner(Margin::new(margin, 0)));
        if self.panel != Panel::None {
            self.draw_panel(frame, state, options, protected);
        }
    }
    fn draw_panel(&mut self, frame: &mut Frame, state: &State, options: &Options, protected: bool) {
        let area = frame.area();
        let width = area.width.saturating_sub(4).min(106);
        let height = area.height.saturating_sub(2).min(match self.panel {
            Panel::Help => 32,
            Panel::Actions => 30,
            Panel::Details => 34,
            Panel::ConfirmResume
            | Panel::ConfirmRollback
            | Panel::ConfirmDiscard
            | Panel::ConfirmPromote => 20,
            _ => 14,
        });
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let accent = match self.panel {
            Panel::ConfirmRollback | Panel::ConfirmDiscard | Panel::ConfirmResume => DANGER,
            Panel::ConfirmPromote => NOTICE,
            _ => FOCUS,
        };
        let block = panel(
            match self.panel {
                Panel::Help => "Keyboard help",
                Panel::Actions => "Actions",
                Panel::Details => "Full target details",
                Panel::ConfirmDiscard => "Discard prepared clone? [Enter] confirm [Esc] cancel",
                Panel::ConfirmRollback => "Rollback dataset [Esc] cancel",
                Panel::ConfirmPromote => "Clone and promote [Enter] confirm [Esc] cancel",
                Panel::ConfirmResume => "Discard saved session? [Enter] continue [Esc] cancel",
                Panel::Unlock => "Unlock boot environment [Enter] unlock [Esc] cancel",
                Panel::None => unreachable!(),
            },
            accent,
        );
        if self.panel == Panel::Actions {
            let selected = self
                .menu
                .selected()
                .and_then(|i| self.menu_commands.get(i))
                .copied();
            let mut commands: Vec<_> = ALL
                .into_iter()
                .filter(|c| {
                    *c != Command::Actions
                        && (!c.needs_saved_session() || state.resume.session_id().is_some())
                })
                .collect();
            commands.sort_by_key(|c| (c.group().0, self.reason(*c, state, options).is_some()));
            self.menu_commands = commands;
            self.menu.select(Some(
                selected
                    .and_then(|c| self.menu_commands.iter().position(|v| *v == c))
                    .unwrap_or(0),
            ));
            let index = self.menu.selected().unwrap_or(0);
            let command = self.menu_commands[index];
            let reason = self.reason(command, state, options);
            let explanation = reason
                .as_ref()
                .map(|r| format!("Unavailable: {}", clean(r)))
                .unwrap_or_else(|| format!("Enter: {}", self.action_label(command, state)));
            let reason_text = Paragraph::new(explanation).wrap(Wrap { trim: false });
            let inner = block.inner(popup);
            // Reserve the explanation even at 60x20. The command list scrolls instead.
            let reason_height = (reason_text.line_count(inner.width) as u16)
                .max(2)
                .min(inner.height.saturating_sub(3));
            let [menu_area, reason_area, hints] = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(reason_height),
                Constraint::Length(1),
            ])
            .areas(inner);
            frame.render_widget(block, popup);
            let mut items = Vec::new();
            let mut previous_group = None;
            let mut selected_row = 0;
            for (i, command) in self.menu_commands.iter().copied().enumerate() {
                let group = command.group();
                if previous_group != Some(group.0) {
                    items.push(ListItem::new(Line::styled(group.1, strong(MUTED))));
                    previous_group = Some(group.0);
                }
                if i == index {
                    selected_row = items.len();
                }
                let disabled = self.reason(command, state, options).is_some();
                items.push(ListItem::new(Line::from(vec![
                    Span::styled(format!("{:12}", command.keys()), ink(MUTED)),
                    Span::styled(
                        self.action_label(command, state),
                        ink(if disabled { MUTED } else { INK }),
                    ),
                    Span::styled(if disabled { "  [unavailable]" } else { "" }, ink(MUTED)),
                ])));
            }
            self.menu_view.select(Some(selected_row));
            frame.render_stateful_widget(
                List::new(items)
                    .highlight_symbol("> ")
                    .highlight_style(ink(BASE).bg(FOCUS).add_modifier(Modifier::BOLD)),
                menu_area,
                &mut self.menu_view,
            );
            frame.render_widget(
                reason_text.style(ink(if reason.is_some() { NOTICE } else { MUTED })),
                reason_area,
            );
            frame.render_widget(
                Paragraph::new(format!(
                    "Up/Down Select  Esc Close   {}/{}",
                    index + 1,
                    self.menu_commands.len()
                ))
                .style(ink(MUTED)),
                hints,
            );
        } else {
            let mut text = match self.panel {
                Panel::ConfirmResume => vec![
                    Line::raw(
                        self.resume_confirmation
                            .as_ref()
                            .map(|(_, command)| self.action_label(*command, state))
                            .unwrap_or_default(),
                    ),
                    Line::raw(""),
                    Line::raw("The saved RAM session will be lost."),
                    Line::raw(match self.resume_confirmation.as_ref().map(|(_, c)| c) {
                        Some(Command::CancelResume) => {
                            "You will stay in the menu. No system will boot."
                        }
                        Some(Command::ColdBoot) => "The selected system will start from scratch.",
                        _ => "The requested action will continue after verified cancellation.",
                    }),
                    Line::raw(""),
                    Line::raw("Enter: discard saved session and continue    Esc: go back"),
                ],
                Panel::Unlock => vec![
                    Line::raw(self.unlock_prompt.as_ref().map(clean).unwrap_or_else(|| {
                        state
                            .environments
                            .get(state.selected_environment)
                            .map(|e| clean(&e.dataset))
                            .unwrap_or_default()
                    })),
                    Line::raw(""),
                    Line::raw("Enter the ZFS encryption passphrase."),
                    Line::raw("Input is hidden and is not saved. Escape cancels."),
                    Line::raw("Passphrase: [hidden]"),
                ],
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
            if self.resume_confirmation.is_some()
                && matches!(
                    self.panel,
                    Panel::ConfirmResume
                        | Panel::ConfirmDiscard
                        | Panel::ConfirmPromote
                        | Panel::ConfirmRollback
                )
            {
                let mut warning = vec![
                    Line::styled(
                        "This action also discards the saved session.",
                        strong(DANGER),
                    ),
                    Line::raw("Unsaved work in running applications will be lost."),
                    Line::raw(""),
                ];
                warning.append(&mut text);
                text = warning;
            }
            let scrollable = matches!(self.panel, Panel::Help | Panel::Details);
            let inner = block.inner(popup);
            let [content, hints] = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(u16::from(scrollable)),
            ])
            .areas(inner);
            let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
            let total = paragraph.line_count(content.width).min(u16::MAX as usize) as u16;
            self.scroll_max = total.saturating_sub(content.height);
            self.scroll = self.scroll.min(self.scroll_max);
            frame.render_widget(block, popup);
            frame.render_widget(paragraph.scroll((self.scroll, 0)), content);
            if scrollable {
                frame.render_widget(
                    Paragraph::new(format!(
                        "Up/Down Scroll  Esc Close   {}-{}/{}",
                        self.scroll + 1,
                        (self.scroll + content.height).min(total),
                        total
                    ))
                    .style(ink(MUTED)),
                    hints,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbm_core::boot::{Backend, SnapshotSource};

    #[test]
    fn footer_describes_search_filters_and_unavailable_targets() {
        let mut state = State {
            pools: zbm_core::preview(),
            ..State::default()
        };
        let mut ui = Ui::default();
        ui.sync(&mut state);
        let options = Options {
            config: crate::config::Config::default(),
            preview: false,
            supervised: true,
            events: crate::diagnostics::Events::default(),
        };
        ui.searching = true;
        ui.input(
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
            &mut state,
        );
        let keys = ui.primary_keys(&state, &options, true).to_string();
        assert!(keys.contains("Enter Keep filter") && keys.contains("Esc Clear"));
        assert!(!keys.contains("Import"));
        ui.input(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
        );
        assert!(
            ui.primary_keys(&state, &options, true)
                .to_string()
                .contains("Esc Clear filter")
        );
        let mut preview = options;
        preview.preview = true;
        assert!(
            ui.primary_keys(&state, &preview, true)
                .to_string()
                .contains("Enter Unavailable")
        );
        ui.open_panel(Panel::Details);
        let keys = ui.primary_keys(&state, &preview, true).to_string();
        assert!(keys.contains("Esc Close") && !keys.contains("Enter"));
    }

    #[test]
    fn clone_summary_tracks_the_selected_generation_and_keeps_paths_in_details() {
        let target = BootTarget {
            dataset: "tank/nixos".into(),
            label: "NixOS (Linux 6.18)".into(),
            root: "/run/snapshot".into(),
            snapshot: Some(SnapshotSource {
                name: "tank/nixos@good".into(),
                guid: 42,
            }),
            mountpoint: None,
            backend: zbm_core::boot::Backend::Nixos(zbm_core::boot::NixosTarget {
                generation: 2,
                specialisation: None,
                toplevel: "/nix/store/system".into(),
                kernel: "/nix/store/kernel/bzImage".into(),
                initrd: Some("/nix/store/initrd/initrd".into()),
                init: "/nix/store/system/init".into(),
                kernel_params: vec![],
            }),
        };
        let mut older = target.clone();
        if let Backend::Nixos(nixos) = &mut older.backend {
            nixos.generation = 1;
        }
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
            events: crate::diagnostics::Events::default(),
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
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 25)).unwrap();
        terminal.draw(|f| ui.draw(f, &state, &options)).unwrap();
        let compact = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(
            compact.contains("Clone prepared.") && compact.contains("snapshot stays unchanged")
        );

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
