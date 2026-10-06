//! One command inventory for dispatch, help, action menus and availability.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Open,
    Back,
    Snapshots,
    Prepare,
    Discard,
    Clone,
    ClonePromote,
    Rollback,
    Rescan,
    Shell,
    Restart,
    PowerOff,
    Exit,
    Search,
    Details,
    Help,
    Actions,
}

pub const ALL: [Command; 17] = [
    Command::Open,
    Command::Back,
    Command::Snapshots,
    Command::Prepare,
    Command::Discard,
    Command::Clone,
    Command::ClonePromote,
    Command::Rollback,
    Command::Rescan,
    Command::Shell,
    Command::Restart,
    Command::PowerOff,
    Command::Exit,
    Command::Search,
    Command::Details,
    Command::Help,
    Command::Actions,
];

impl Command {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open / Boot",
            Self::Back => "Back",
            Self::Snapshots => "Snapshots",
            Self::Prepare => "Prepare / reuse snapshot clone",
            Self::Discard => "Discard prepared clone",
            Self::Clone => "Clone snapshot as boot environment",
            Self::ClonePromote => "Clone snapshot and promote",
            Self::Rollback => "Roll back dataset to snapshot",
            Self::Rescan => "Rescan",
            Self::Shell => "Recovery shell",
            Self::Restart => "Restart manager",
            Self::PowerOff => "Power off",
            Self::Exit => "Exit manager",
            Self::Search => "Find in current list",
            Self::Details => "Full target details",
            Self::Help => "Keyboard help",
            Self::Actions => "All actions",
        }
    }

    pub fn keys(self) -> &'static str {
        match self {
            Self::Open => "Enter",
            Self::Back => "B / Esc",
            Self::Snapshots => "T",
            Self::Prepare => "C",
            Self::Discard => "D",
            Self::Clone => "O",
            Self::ClonePromote => "M",
            Self::Rollback => "U",
            Self::Rescan => "R / F5",
            Self::Shell => "S / F4",
            Self::Restart => "N / F6",
            Self::PowerOff => "P / F10",
            Self::Exit => "Q",
            Self::Search => "/",
            Self::Details => "I / F3",
            Self::Help => "? / F1",
            Self::Actions => "F2",
        }
    }

    pub fn recovery(self) -> bool {
        matches!(
            self,
            Self::Shell | Self::Restart | Self::PowerOff | Self::Exit
        )
    }

    pub fn from_key(key: KeyEvent) -> Option<Self> {
        if key.modifiers.intersects(
            KeyModifiers::CONTROL
                | KeyModifiers::ALT
                | KeyModifiers::SUPER
                | KeyModifiers::HYPER
                | KeyModifiers::META,
        ) {
            return None;
        }
        Some(match key.code {
            KeyCode::Enter => Self::Open,
            KeyCode::Esc => Self::Back,
            KeyCode::F(1) => Self::Help,
            KeyCode::F(2) => Self::Actions,
            KeyCode::F(3) => Self::Details,
            KeyCode::F(4) => Self::Shell,
            KeyCode::F(5) => Self::Rescan,
            KeyCode::F(6) => Self::Restart,
            KeyCode::F(10) => Self::PowerOff,
            KeyCode::Char(c) => match c.to_ascii_lowercase() {
                'b' => Self::Back,
                't' => Self::Snapshots,
                'c' => Self::Prepare,
                'd' => Self::Discard,
                'o' => Self::Clone,
                'm' => Self::ClonePromote,
                'u' => Self::Rollback,
                'r' => Self::Rescan,
                's' => Self::Shell,
                'n' => Self::Restart,
                'p' => Self::PowerOff,
                'q' => Self::Exit,
                '/' => Self::Search,
                'i' => Self::Details,
                '?' => Self::Help,
                _ => return None,
            },
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modifiers_never_trigger_bare_mutation_or_power_shortcuts() {
        for key in ['p', 'c', 'd', 's', 'n', 'q'] {
            assert_eq!(
                Command::from_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL)),
                None
            );
            assert_eq!(
                Command::from_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::ALT)),
                None
            );
        }
        assert_eq!(
            Command::from_key(KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE)),
            Some(Command::Shell)
        );
    }
}
