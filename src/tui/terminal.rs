use std::io::{self, Stdout};

use anyhow::Result;
use crossterm::{
    cursor, execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

pub type OrcTerminal = Terminal<CrosstermBackend<Stdout>>;

pub struct TerminalSession {
    terminal: OrcTerminal,
}

impl TerminalSession {
    pub fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, cursor::Hide) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self { terminal })
    }

    pub fn terminal(&mut self) -> &mut OrcTerminal {
        &mut self.terminal
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        restore();
    }
}

pub fn install_panic_restore_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
}

fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), cursor::Show, LeaveAlternateScreen);
}
