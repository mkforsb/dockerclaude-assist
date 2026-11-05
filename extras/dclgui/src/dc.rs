//! Running `dockerclaude.sh` and other external programs.

use std::env;
use std::fmt;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{self, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use iced::futures::future::join_all;

use crate::parse::{self, Instance, Issue};

/// Set for child processes, so that sudo running this program as its askpass
/// helper gets the password prompt rather than the main window.
pub const ASKPASS_ENV: &str = "DCLGUI_ASKPASS";

const TERMINAL: &str = "x-terminal-emulator";

/// A `dockerclaude.sh` command for the instance with the given prefix.
#[derive(Debug, Clone, PartialEq)]
pub struct Invocation {
    pub prefix: String,
    pub args: Vec<String>,
}

impl Invocation {
    pub fn new<S: Into<String>>(prefix: &str, args: impl IntoIterator<Item = S>) -> Self {
        Self {
            prefix: prefix.to_owned(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

impl fmt::Display for Invocation {
    /// Formats as a shell command.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.prefix.is_empty() {
            write!(f, "DCPREFIX={} ", shell_quote(&self.prefix))?;
        }

        write!(f, "dockerclaude")?;

        for arg in &self.args {
            write!(f, " {}", shell_quote(arg))?;
        }

        Ok(())
    }
}

fn shell_quote(word: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "-_./=:".contains(c);

    if !word.is_empty() && word.chars().all(safe) {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

#[derive(Debug, Clone)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Finds the program `name` on `PATH`.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

/// Finds `dockerclaude.sh`, in order: `$DCLGUI_SCRIPT`, the directories above
/// this executable, i.e. the checkout it was built in, and `PATH`.
pub fn find_script() -> Option<PathBuf> {
    if let Some(script) = env::var_os("DCLGUI_SCRIPT") {
        return Some(script.into());
    }

    let exe = env::current_exe().ok()?;

    exe.ancestors()
        .skip(1)
        .map(|dir| dir.join("dockerclaude.sh"))
        .find(|path| path.is_file())
        .or_else(|| find_on_path("dockerclaude"))
        .or_else(|| find_on_path("dockerclaude.sh"))
}

/// Detaches from the controlling terminal, if any. Otherwise sudo would ask for
/// its password on the terminal the GUI was started from, instead of using the
/// askpass helper.
fn detach(command: &mut process::Command) {
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Runs a `dockerclaude.sh` command to completion.
pub async fn run(script: Arc<PathBuf>, invocation: Invocation) -> Result<Output, String> {
    let mut command = process::Command::new(script.as_path());

    command
        .args(&invocation.args)
        .env("DCPREFIX", &invocation.prefix)
        .env(ASKPASS_ENV, "1")
        .stdin(Stdio::null());

    if env::var_os("SUDO_ASKPASS").is_none()
        && let Ok(exe) = env::current_exe()
    {
        command.env("SUDO_ASKPASS", exe);
    }

    detach(&mut command);

    let output = tokio::process::Command::from(command)
        .output()
        .await
        .map_err(|error| format!("Failed to run {}: {error}", script.display()))?;

    Ok(Output {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Gathers everything shown on the Overview tab.
pub async fn load_overview(script: Arc<PathBuf>) -> Result<Vec<Instance>, String> {
    let (ps, snapshots) = iced::futures::join!(
        run(script.clone(), Invocation::new("", ["ps", "-a"])),
        run(script.clone(), Invocation::new("", ["snapshots-list"])),
    );

    let mut instances = parse::parse_ps(&ps?.stdout);
    let mut snapshots = parse::parse_snapshots(&snapshots?.stdout);

    for instance in &mut instances {
        instance.snapshot_bytes = snapshots.remove(&instance.prefix);
    }

    for (prefix, size) in snapshots {
        instances.push(Instance {
            prefix,
            snapshot_bytes: Some(size),
            ..Instance::default()
        });
    }

    let histories = join_all(instances.iter().map(|instance| {
        run(
            script.clone(),
            Invocation::new(&instance.prefix, ["history"]),
        )
    }))
    .await;

    for (instance, history) in instances.iter_mut().zip(histories) {
        instance.last_prompt = history?.stdout.lines().last().map(str::to_owned);
    }

    Ok(instances)
}

/// Gathers everything shown on the Doctor tab.
pub async fn load_issues(script: Arc<PathBuf>) -> Result<Vec<Issue>, String> {
    // Fails if there are issues, so that says nothing about the output.
    let doctor = run(script, Invocation::new("", ["doctor"])).await?;

    if doctor.stdout.trim().is_empty() {
        return Err(format!(
            "dockerclaude doctor failed: {}",
            doctor.stderr.trim()
        ));
    }

    let mut issues = parse::parse_doctor(&doctor.stdout);

    if find_on_path(TERMINAL).is_none() {
        issues.push(Issue {
            description: format!("{TERMINAL} not found, the Bash button will not work"),
            fix: None,
        });
    }

    Ok(issues)
}

/// Set when rfd reports having no way of showing a file chooser. It tells callers
/// nothing but "no directory was picked", the same as if the user cancelled.
static PICKER_FAILED: AtomicBool = AtomicBool::new(false);

struct PickerFailureLog;

impl log::Log for PickerFailureLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() == log::Level::Error && metadata.target().starts_with("rfd")
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            PICKER_FAILED.store(true, Ordering::Relaxed);
        }
    }

    fn flush(&self) {}
}

/// Lets [`pick_directory`] tell a failure from the user cancelling.
pub fn watch_for_picker_failures() {
    if log::set_logger(&PickerFailureLog).is_ok() {
        log::set_max_level(log::LevelFilter::Error);
    }
}

/// Asks the user for a directory, by way of the XDG desktop portal or else zenity.
/// Returns `None` if the user cancels.
pub async fn pick_directory() -> Result<Option<String>, String> {
    let directory = rfd::AsyncFileDialog::new()
        .set_title("Choose a directory to mount")
        .pick_folder()
        .await;

    if directory.is_none() && PICKER_FAILED.swap(false, Ordering::Relaxed) {
        return Err("No file chooser available, install xdg-desktop-portal-gtk or zenity".into());
    }

    Ok(directory.map(|directory| directory.path().to_string_lossy().into_owned()))
}

/// Opens a terminal running `dockerclaude bash` for the given prefix.
///
/// The command is passed as separate arguments, the one form of `-e` that
/// Debian policy requires of every x-terminal-emulator, and with the full path
/// to the script, as `dockerclaude` is at best a shell function.
pub fn open_bash(script: &Path, prefix: &str) -> Result<(), String> {
    let mut command = process::Command::new(TERMINAL);

    command
        .arg("-e")
        .arg("env")
        .arg(format!("DCPREFIX={prefix}"))
        .arg(script)
        .arg("bash")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    detach(&mut command);

    let mut child = command
        .spawn()
        .map_err(|error| format!("Failed to run {TERMINAL}: {error}"))?;

    // Reap it once the user closes the terminal.
    std::thread::spawn(move || child.wait());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocations_display_as_shell_commands() {
        assert_eq!(
            Invocation::new("", ["build-image"]).to_string(),
            "dockerclaude build-image"
        );
        assert_eq!(
            Invocation::new("work-", ["mount", "/tmp/a b", "it's"]).to_string(),
            r"DCPREFIX=work- dockerclaude mount '/tmp/a b' 'it'\''s'"
        );
    }
}
