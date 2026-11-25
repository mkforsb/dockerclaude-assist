//! Terminal output: colored status lines, key/value summaries and the
//! confirmation prompt shared by every subcommand that takes `--yes`.

use std::fmt::Display;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};

use anyhow::{Result, bail};
use owo_colors::{OwoColorize, Stream};

/// `==> message`, for the main steps of a command.
pub fn step(msg: impl Display) {
    eprintln!(
        "{} {}",
        "==>".if_supports_color(Stream::Stderr, |s| s.blue().bold().to_string()),
        msg.if_supports_color(Stream::Stderr, |s| s.bold().to_string())
    );
}

/// `  ✓ message`, for something that is done (or already was).
pub fn ok(msg: impl Display) {
    eprintln!("  {} {}", "✓".if_supports_color(Stream::Stderr, |s| s.green().to_string()), msg);
}

/// `  · message`, for side notes.
pub fn note(msg: impl Display) {
    eprintln!("  {} {}", "·".if_supports_color(Stream::Stderr, |s| s.dimmed().to_string()), msg);
}

pub fn warn(msg: impl Display) {
    eprintln!("{} {}", "warning:".if_supports_color(Stream::Stderr, |s| s.yellow().bold().to_string()), msg);
}

pub fn error(msg: impl Display) {
    eprintln!("{} {}", "error:".if_supports_color(Stream::Stderr, |s| s.red().bold().to_string()), msg);
}

/// Prints a command line about to be run, dimmed.
pub fn cmdline(args: &[String]) {
    let line = args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
    eprintln!("  {}", format!("$ {line}").if_supports_color(Stream::Stderr, |s| s.dimmed().to_string()));
}

pub fn bold(s: impl Display) -> String {
    s.if_supports_color(Stream::Stdout, |s| s.bold().to_string()).to_string()
}

pub fn dim(s: impl Display) -> String {
    s.if_supports_color(Stream::Stdout, |s| s.dimmed().to_string()).to_string()
}

pub fn green(s: impl Display) -> String {
    s.if_supports_color(Stream::Stdout, |s| s.green().to_string()).to_string()
}

pub fn yellow(s: impl Display) -> String {
    s.if_supports_color(Stream::Stdout, |s| s.yellow().to_string()).to_string()
}

pub fn red(s: impl Display) -> String {
    s.if_supports_color(Stream::Stdout, |s| s.red().to_string()).to_string()
}

pub fn cyan(s: impl Display) -> String {
    s.if_supports_color(Stream::Stdout, |s| s.cyan().to_string()).to_string()
}

/// Quotes `s` for a POSIX shell, only when needed.
pub fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%^".contains(c));
    if safe { s.to_string() } else { format!("'{}'", s.replace('\'', r"'\''")) }
}

/// What a confirmation prompt shows: a headline, then aligned `key  value`
/// rows, then free-form notes (warnings, consequences).
pub struct Plan {
    title: String,
    rows: Vec<(String, String)>,
    notes: Vec<String>,
}

impl Plan {
    pub fn new(title: impl Into<String>) -> Self {
        Self { title: title.into(), rows: Vec::new(), notes: Vec::new() }
    }

    pub fn row(mut self, key: impl Into<String>, value: impl Display) -> Self {
        self.rows.push((key.into(), value.to_string()));
        self
    }

    pub fn row_if(self, cond: bool, key: impl Into<String>, value: impl Display) -> Self {
        if cond { self.row(key, value) } else { self }
    }

    /// Adds a note; empty ones are skipped, for conditional notes.
    pub fn note(mut self, note: impl Into<String>) -> Self {
        let note = note.into();
        if !note.is_empty() {
            self.notes.push(note);
        }
        self
    }

    pub fn print(&self) {
        let width = self.rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
        eprintln!("{}", self.title.if_supports_color(Stream::Stderr, |s| s.bold().to_string()));
        for (k, v) in &self.rows {
            let pad = " ".repeat(width - k.chars().count());
            let mut lines = v.lines();
            let first = lines.next().unwrap_or("");
            eprintln!("  {}{pad}  {first}", k.if_supports_color(Stream::Stderr, |s| s.cyan().to_string()));
            for line in lines {
                eprintln!("  {}  {line}", " ".repeat(width));
            }
        }
        for n in &self.notes {
            eprintln!("  {} {n}", "!".if_supports_color(Stream::Stderr, |s| s.yellow().bold().to_string()));
        }
    }

    /// Shows the plan and asks to go ahead, unless `yes`. Declining is an
    /// error, so that the caller stops (and scripts see a failure).
    pub fn confirm(&self, yes: bool) -> Result<()> {
        if yes {
            return Ok(());
        }
        self.print();
        if !ask("Proceed?")? {
            bail!(Declined);
        }
        Ok(())
    }
}

/// Raised when the user answers no; `main` prints it without the `error:` look.
#[derive(Debug)]
pub struct Declined;

impl std::fmt::Display for Declined {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Declined {}

/// Asks a yes/no question on the terminal (defaulting to no). Reads from
/// /dev/tty so that it works with stdin redirected, and fails instead of
/// hanging when there is no terminal at all.
pub fn ask(question: &str) -> Result<bool> {
    let tty = OpenOptions::new().read(true).write(true).open("/dev/tty");
    let Ok(tty) = tty else {
        bail!("confirmation needed but there is no terminal to ask on; pass --yes to proceed without asking");
    };
    let mut out = &tty;
    write!(
        out,
        "{} {} ",
        question.if_supports_color(Stream::Stderr, |s| s.bold().to_string()),
        "[y/N]".if_supports_color(Stream::Stderr, |s| s.dimmed().to_string())
    )?;
    out.flush()?;
    let mut line = String::new();
    BufReader::new(&tty).read_line(&mut line)?;
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}
