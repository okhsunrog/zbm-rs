//! Rendering and input transport; the manager owns policy and all operations.
use super::{
    Options,
    ui::{Input, Ui},
};
use anyhow::Result;
use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use zbm_core::State;

pub(crate) enum FrontendEvent {
    Key(KeyEvent),
    Resize,
    #[cfg(feature = "gui-prototype")]
    Gui(crate::gui::Event),
}

impl FrontendEvent {
    pub fn apply(self, ui: &mut Ui, state: &mut State) -> Input {
        match self {
            Self::Key(key) => ui.input(key, state),
            Self::Resize => Input::Ignored,
            #[cfg(feature = "gui-prototype")]
            Self::Gui(event) => ui.gui_input(event, state),
        }
    }
}

pub(crate) enum Frontend {
    Terminal(Option<ratatui::DefaultTerminal>),
    #[cfg(feature = "gui-prototype")]
    Gui(crate::gui::Transport),
}

impl Frontend {
    pub fn terminal() -> Self {
        Self::Terminal(Some(ratatui::init()))
    }
    pub fn draw(&mut self, state: &State, options: &Options, ui: &mut Ui) -> Result<()> {
        match self {
            Self::Terminal(terminal) => {
                terminal
                    .as_mut()
                    .unwrap()
                    .draw(|frame| ui.draw(frame, state, options))?;
            }
            #[cfg(feature = "gui-prototype")]
            Self::Gui(transport) => transport.draw(ui, state, options)?,
        }
        Ok(())
    }
    pub fn poll(&mut self) -> Result<Option<FrontendEvent>> {
        match self {
            Self::Terminal(_) => Ok(if event::poll(std::time::Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        Some(FrontendEvent::Key(key))
                    }
                    Event::Resize(..) => Some(FrontendEvent::Resize),
                    _ => None,
                }
            } else {
                None
            }),
            #[cfg(feature = "gui-prototype")]
            Self::Gui(transport) => transport.poll().map(|event| event.map(FrontendEvent::Gui)),
        }
    }
    pub fn suspend(&mut self) -> Result<()> {
        match self {
            Self::Terminal(terminal) => {
                if terminal.take().is_some() {
                    ratatui::restore();
                }
            }
            #[cfg(feature = "gui-prototype")]
            Self::Gui(transport) => transport.hide()?,
        }
        Ok(())
    }
    pub fn resume(&mut self) -> Result<()> {
        match self {
            Self::Terminal(terminal) => *terminal = Some(ratatui::init()),
            #[cfg(feature = "gui-prototype")]
            Self::Gui(_) => anyhow::bail!("Graphical recovery requires the supervisor"),
        }
        Ok(())
    }
}
impl Drop for Frontend {
    fn drop(&mut self) {
        let _ = self.suspend();
    }
}
