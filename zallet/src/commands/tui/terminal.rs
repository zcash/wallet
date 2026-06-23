//! Terminal setup and teardown for the TUI.

use std::io::{self, Stdout};

use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

/// Alias for the concrete terminal type used by the TUI.
pub(super) type Tui = Terminal<CrosstermBackend<Stdout>>;

/// An RAII guard that places the terminal into raw mode and the alternate screen on
/// construction, and restores it on drop.
///
/// Restoring on drop ensures the user's terminal is not left in a broken state if the UI
/// panics. [`TerminalGuard::restore`] can be called explicitly to restore early (e.g. so
/// that an error message is printed to a normal terminal); it is idempotent.
pub(super) struct TerminalGuard {
    terminal: Option<Tui>,
    restored: bool,
}

impl TerminalGuard {
    /// Enters raw mode and the alternate screen, returning a guard that owns the terminal.
    ///
    /// If setup fails partway through (e.g. entering the alternate screen or constructing
    /// the terminal fails after raw mode was enabled), any state already changed is rolled
    /// back before returning the error, so the user's terminal is not left in raw mode.
    pub(super) fn enter() -> io::Result<Self> {
        enable_raw_mode()?;

        // From here on, undo raw mode (and the alternate screen) if anything fails, since
        // no `Drop` guard exists yet to restore it.
        let setup = (|| {
            let mut stdout = io::stdout();
            execute!(stdout, EnterAlternateScreen)?;
            let backend = CrosstermBackend::new(stdout);
            Terminal::new(backend)
        })();

        match setup {
            Ok(terminal) => Ok(Self {
                terminal: Some(terminal),
                restored: false,
            }),
            Err(e) => {
                // Best-effort rollback; preserve and return the original error.
                let _ = disable_raw_mode();
                let _ = execute!(io::stdout(), LeaveAlternateScreen);
                Err(e)
            }
        }
    }

    /// Returns a mutable reference to the underlying terminal.
    pub(super) fn terminal_mut(&mut self) -> &mut Tui {
        self.terminal
            .as_mut()
            .expect("terminal is present until the guard is dropped")
    }

    /// Restores the terminal to its original state. Idempotent.
    pub(super) fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;

        // Best-effort restoration; nothing useful can be done if these fail.
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        if let Some(terminal) = self.terminal.as_mut() {
            let _ = terminal.show_cursor();
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}
