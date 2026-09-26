//! When to use color and Unicode, and which stream each line goes to.
//!
//! - **Data on stdout, messages on stderr.** `nord program get 7:4 | grep transpose` must
//!   see only the summary, so every progress line, warning and pre-flight description
//!   goes to stderr.
//! - **Color and Unicode only on a TTY.** ⚠️ The cross-platform check compares what
//!   `nord.exe` under Wine and the native Linux binary print for the same input. An
//!   escape sequence or box-drawing character in piped output would make that
//!   comparison depend on Wine's console code page.
//! - **Without a TTY, never prompt.** Do not read a stdin nobody is attached to, and do
//!   not proceed just because nobody is there to say no.

use std::fmt::Display;
use std::io::{BufRead, IsTerminal, Write};

/// When to emit ANSI color.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ColorChoice {
    /// Color when stdout is a terminal, `NO_COLOR` is unset, and `TERM` is not `dumb`.
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
            // `NO_COLOR` counts whatever its value: by convention, being set is the
            // signal.
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

    /// What a redirected run gets: no color, no Unicode, and no prompts. Tests use it so
    /// they do not depend on the terminal they were started from.
    #[cfg(test)]
    pub(crate) fn piped() -> Ui {
        Ui {
            color: false,
            unicode: false,
            interactive: false,
        }
    }

    /// Data. The only output that goes to stdout.
    ///
    /// ⚠️ Not `println!`, which panics when the reader goes away, so `nord program edit
    /// --fields | head` would print a backtrace. A closed pipe ends the run silently but
    /// with a failure status: every mutation prints what it will change here before it
    /// writes, so exiting 0 would report an edit that never happened.
    pub fn out(&self, line: impl Display) {
        let mut stdout = std::io::stdout().lock();
        if let Err(e) = writeln!(stdout, "{line}") {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(BROKEN_PIPE);
            }
            eprintln!("writing to stdout: {e}");
            std::process::exit(1);
        }
    }

    /// Progress, pre-flight descriptions, and anything else a pipe should not see.
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
    /// ⚠️ A caller that draws data as a glyph must also print that data in the plain
    /// form, so a pipe gets as much information as the terminal.
    pub fn unicode(&self) -> bool {
        self.unicode
    }

    /// A section heading inside a summary.
    ///
    /// Color has three meanings: a heading, a dimmed label or inactive value, and
    /// [`Ui::danger`] for something about to be destroyed.
    ///
    /// ⚠️ A heading and a danger are both red and differ only in weight. They stay
    /// distinct because they never share a stream: headings are data on stdout, and
    /// dangers go to stderr just above a prompt.
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

    /// Confirm a destructive action: `--yes` was given, or the user agrees at a prompt.
    ///
    /// ⚠️ The caller must already have described what will change. This only asks.
    ///
    /// Without a TTY, a missing `--yes` is an error, so the run never waits on a stdin
    /// that may never produce a line.
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

    /// Ask for a line of text, with `initial` already in the buffer to edit. `None`
    /// means the user entered an empty line to finish.
    ///
    /// ⚠️ This takes the terminal out of line mode to do its own editing, so it must
    /// never run against a stream that is not a terminal. Without a TTY it returns an
    /// error.
    ///
    /// ⚠️ Ctrl-C never returns here. The raw-mode reader raises `SIGINT` on itself, so an
    /// interrupt at the prompt ends the process. A caller cannot clean up after one, and
    /// must leave nothing that needs undoing while this waits. Ctrl-D, Ctrl-U and Esc
    /// are all read as nothing.
    pub fn ask(&self, question: &str, initial: &str) -> Result<Option<String>, String> {
        if !self.interactive {
            return Err(format!("{question:?} needs a terminal to ask on"));
        }
        // Reads and echoes on stderr, like every other prompt.
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

/// What a shell reports for a process `SIGPIPE` killed: 128 plus the signal's number.
const BROKEN_PIPE: i32 = 141;

const BOLD: &str = "1";
const BOLD_RED: &str = "1;31";
const DIM: &str = "2";
const RED: &str = "31";
const YELLOW: &str = "33";

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Scripts rely on this refusal for safety, so it must not depend on reaching a
    /// prompt.
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

    /// An open question has no `--yes` to answer it, so without a TTY it must fail
    /// instead of waiting for a line.
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
