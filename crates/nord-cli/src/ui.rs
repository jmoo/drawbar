//! Presentation gating: color, unicode, and which stream a line belongs on.
//!
//! - **Data on stdout, chatter on stderr.** `nord program get 7:4 | grep transpose` must
//!   see the summary and nothing else, so every progress line, warning and pre-flight
//!   description goes to stderr.
//! - **Color and unicode only on a TTY.** ⚠️ The cross-platform check compares the bytes
//!   a Wine-hosted `nord.exe` and the native Linux binary print for the same input, and
//!   an escape sequence or box-drawing character that survived a pipe would put that
//!   comparison at the mercy of Wine's console codepage.
//! - **A non-TTY is non-interactive.** Never read a stdin nobody is attached to, and
//!   never auto-proceed because nobody is there to say no.

use std::fmt::Display;
use std::io::{BufRead, IsTerminal, Write};

/// When to emit ANSI color.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ColorChoice {
    /// Color when stdout is a terminal and `NO_COLOR` is unset.
    #[default]
    Auto,
    Always,
    Never,
}

/// What this invocation may print, and to whom.
#[derive(Copy, Clone, Debug)]
pub struct Ui {
    color: bool,
    unicode: bool,
    interactive: bool,
}

impl Ui {
    pub fn new(choice: ColorChoice) -> Self {
        let tty = std::io::stdout().is_terminal();
        let color = match choice {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            // `NO_COLOR` is honored whatever its value; the convention is that the
            // variable being present at all is the signal.
            ColorChoice::Auto => {
                tty && std::env::var_os("NO_COLOR").is_none()
                    && std::env::var("TERM").as_deref() != Ok("dumb")
            }
        };
        Ui {
            color,
            // Not tied to `--color`: `--color=always` into a file still gets ASCII.
            unicode: tty,
            // stderr as well as stdin: the prompt is written to stderr, so a redirected
            // stderr means the question would never be seen.
            interactive: std::io::stdin().is_terminal() && std::io::stderr().is_terminal(),
        }
    }

    /// Data. Goes to stdout, and is the only thing that does.
    ///
    /// ⚠️ Not `println!`, which **panics** when the reader goes away: `nord program edit
    /// --fields | head` would print a Rust backtrace over the user's terminal. A closed
    /// pipe exits successfully and silently instead.
    pub fn out(&self, line: impl Display) {
        let mut stdout = std::io::stdout().lock();
        if let Err(e) = writeln!(stdout, "{line}") {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
            eprintln!("writing to stdout: {e}");
            std::process::exit(1);
        }
    }

    /// Chatter — progress, pre-flight descriptions, anything a pipe should not see.
    pub fn note(&self, line: impl Display) {
        eprintln!("{line}");
    }

    pub fn warn(&self, line: impl Display) {
        eprintln!("{}{line}", self.style("warning: ", YELLOW));
    }

    /// An em dash on a terminal, a hyphen everywhere else.
    pub fn dash(&self) -> &'static str {
        if self.unicode {
            "—"
        } else {
            "-"
        }
    }

    /// Whether box-drawing and block glyphs may be used.
    ///
    /// ⚠️ A caller that substitutes a glyph for data must keep the plain form carrying
    /// that data too — a pipe has to stay as informative as the terminal.
    pub fn unicode(&self) -> bool {
        self.unicode
    }

    /// A section heading inside a summary.
    ///
    /// Color carries three meanings and no more: a heading, a dimmed label or inactive
    /// value, and [`Ui::danger`] for something about to be destroyed.
    ///
    /// ⚠️ A heading and a danger are both red, separated only by weight. They stay
    /// distinguishable by never sharing a stream — headings are data on stdout, dangers
    /// are chatter on stderr immediately above a prompt.
    pub fn heading(&self, s: impl Display) -> String {
        self.style(s, BOLD_RED)
    }

    pub fn bold(&self, s: impl Display) -> String {
        self.style(s, BOLD)
    }

    pub fn dim(&self, s: impl Display) -> String {
        self.style(s, DIM)
    }

    pub fn danger(&self, s: impl Display) -> String {
        self.style(s, RED)
    }

    fn style(&self, s: impl Display, code: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    /// Settle a destructive action: `--yes` given, or the operator says so at a prompt.
    ///
    /// ⚠️ The caller must already have described what will be touched. This asks the
    /// question; it does not state the stakes.
    ///
    /// Off a TTY a missing `--yes` is an error rather than a block on a stdin that may
    /// never produce a line.
    pub fn confirm(&self, already: bool) -> Result<(), String> {
        if already {
            return Ok(());
        }
        if !self.interactive {
            return Err(format!(
                "refusing to proceed without {}",
                self.bold("--yes")
            ));
        }
        eprint!("{} [y/N] ", self.danger("proceed?"));
        std::io::stderr().flush().ok();
        let mut answer = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut answer)
            .map_err(|e| format!("reading the answer: {e}"))?;
        match answer.trim() {
            "y" | "Y" | "yes" => Ok(()),
            _ => Err("canceled".into()),
        }
    }

    /// Ask for a line of free text, with `initial` already in the buffer and editable.
    /// `None` is the operator saying they are done — an empty line, and only that.
    ///
    /// ⚠️ This takes the terminal out of line mode to do its own editing, so unlike
    /// [`Ui::note`] it must never run against a stream that is not a terminal. Off a TTY
    /// it is an error rather than a wait on a stdin nobody is attached to.
    ///
    /// ⚠️ **Ctrl-C never arrives here.** The raw-mode reader raises `SIGINT` on itself,
    /// so an interrupt at the prompt ends the *process*: a caller cannot run any cleanup
    /// after one, and must leave nothing that needs undoing while this is waiting. Ctrl-D,
    /// Ctrl-U and Esc are all read as nothing at all.
    pub fn ask(&self, question: &str, initial: &str) -> Result<Option<String>, String> {
        if !self.interactive {
            return Err(format!("{question:?} needs a terminal to ask on"));
        }
        // Reads and echoes on stderr, which is where every other prompt here goes.
        let answer = dialoguer::Input::<String>::new()
            .with_prompt(question)
            .with_initial_text(initial)
            .allow_empty(true)
            .interact_text()
            .map_err(|e| format!("reading the answer: {e}"))?;
        let answer = answer.trim();
        Ok((!answer.is_empty()).then(|| answer.to_string()))
    }
}

const BOLD: &str = "1";
const BOLD_RED: &str = "1;31";
const DIM: &str = "2";
const RED: &str = "31";
const YELLOW: &str = "33";

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Ui` built for a pipe styles nothing, whatever it is asked to style.
    #[test]
    fn without_a_tty_nothing_is_decorated() {
        let ui = Ui {
            color: false,
            unicode: false,
            interactive: false,
        };
        assert_eq!(ui.bold("x"), "x");
        assert_eq!(ui.danger("x"), "x");
        assert_eq!(ui.heading("x"), "x");
        assert_eq!(ui.dim("x"), "x");
        assert_eq!(ui.dash(), "-");
        assert!(!ui.unicode());
    }

    #[test]
    fn with_color_the_reset_always_closes_the_sequence() {
        let ui = Ui {
            color: true,
            unicode: true,
            interactive: false,
        };
        assert_eq!(ui.bold("x"), "\x1b[1mx\x1b[0m");
        assert_eq!(ui.heading("x"), "\x1b[1;31mx\x1b[0m");
        assert_eq!(ui.dash(), "—");
        assert!(ui.unicode());
    }

    /// The non-interactive refusal is the whole safety story for scripts, so it must not
    /// depend on reaching a prompt.
    #[test]
    fn a_pipe_without_yes_is_refused_rather_than_asked() {
        let ui = Ui {
            color: false,
            unicode: false,
            interactive: false,
        };
        assert!(ui.confirm(true).is_ok());
        let err = ui.confirm(false).unwrap_err();
        assert!(err.contains("--yes"), "{err}");
    }

    /// An open-ended question has no `--yes` to stand in for an answer, so off a TTY it
    /// can only fail — never block waiting for a line.
    #[test]
    fn a_pipe_is_never_asked_an_open_question() {
        let ui = Ui {
            color: false,
            unicode: false,
            interactive: false,
        };
        let err = ui.ask("what changed", "").unwrap_err();
        assert!(err.contains("terminal"), "{err}");
    }
}
