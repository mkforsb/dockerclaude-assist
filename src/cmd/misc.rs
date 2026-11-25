//! install, exec, dir, dot-claude-clean.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::docker;
use crate::mounts::{self, Visibility};
use crate::paths;
use crate::space::{container_name, require_running};
use crate::ui::{self, Plan};

/// `docker exec` with the terminal variables, `-i` always and `-t` when
/// stdin is a terminal.
pub fn exec_cmd(name: &str, workdir: Option<&Path>) -> std::process::Command {
    let mut c = docker::cmd(["exec", "-i"]);
    if std::io::stdin().is_terminal() {
        c.arg("-t");
    }
    if let Some(w) = workdir {
        c.arg("-w").arg(w);
    }
    c.args(docker::terminal_env_args()).arg(name);
    c
}

/// The container path of the current directory, if it is visible in the
/// container (mounted itself, or inside a mounted directory).
pub fn cwd_in_container(name: &str, pid: u32) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?.canonicalize().ok()?;
    let current = mounts::list(name, pid).ok()?;
    match mounts::visibility(&cwd, &current) {
        Visibility::Mounted(_) | Visibility::Inside(_) => Some(mounts::target_for(&cwd)),
        _ => None,
    }
}

pub fn install(space: &str, names: &[String], yes: bool) -> Result<i32> {
    let name = container_name(space);
    require_running(&docker::container(&name)?, &name)?;
    let dir = paths::installers_dir()?;
    let (installers, packages): (Vec<&String>, Vec<&String>) =
        names.iter().partition(|n| dir.join(format!("{n}.sh")).is_file());

    Plan::new(format!("Install into container {}", ui::bold(&name)))
        .row_if(
            !installers.is_empty(),
            "installers",
            installers
                .iter()
                .map(|n| paths::display(&dir.join(format!("{n}.sh"))))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .row_if(
            !packages.is_empty(),
            "apt packages",
            packages.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" "),
        )
        .row("as", "root, umask 002")
        .note(if packages.is_empty() || !installers.is_empty() {
            String::new()
        } else {
            format!("No installer by these names in {}, so they go to apt", paths::display(&dir))
        })
        .confirm(yes)?;

    for n in &installers {
        let src = dir.join(format!("{n}.sh"));
        let dst = format!("/tmp/dockerclaude-installer-{n}.sh");
        ui::step(format!("Running installer {n}"));
        let mut cp = docker::cmd(["cp"]);
        cp.arg(&src).arg(format!("{name}:{dst}"));
        docker::output(cp)?;
        let mut run = exec_cmd(&name, None);
        run.args([
            "bash",
            "-lc",
            r#"umask 002 && chmod 755 "$1" && "$1"; rc=$?; rm -f "$1"; exit $rc"#,
            "bash",
            &dst,
        ]);
        let status = docker::run_status(run)?;
        if !status.success() {
            ui::error(format!("installer {n} failed ({})", docker::exit_text(status)));
            return Ok(status.code().unwrap_or(1));
        }
        ui::ok(format!("installer {n} done"));
    }
    if !packages.is_empty() {
        ui::step(format!(
            "apt install {}",
            packages.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ")
        ));
        let mut run = exec_cmd(&name, None);
        run.args(["bash", "-lc", r#"umask 002 && apt-get update && apt-get install -y "$@""#, "bash"]);
        run.args(packages.iter().map(|s| s.as_str()));
        let status = docker::run_status(run)?;
        if !status.success() {
            ui::error(format!("apt install failed ({})", docker::exit_text(status)));
            return Ok(status.code().unwrap_or(1));
        }
        ui::ok("packages installed");
    }
    Ok(0)
}

pub fn exec(space: &str, command: &[String], yes: bool) -> Result<i32> {
    let name = container_name(space);
    let c = require_running(&docker::container(&name)?, &name)?;
    let workdir = cwd_in_container(&name, c.pid);

    Plan::new(format!("Run in container {}", ui::bold(&name)))
        .row("command", command.iter().map(|a| ui::shell_quote(a)).collect::<Vec<_>>().join(" "))
        .row("in", workdir.as_ref().map(|w| w.display().to_string()).unwrap_or_else(|| "/workspace".into()))
        .row("as", "root, umask 002")
        .confirm(yes)?;

    let mut run = exec_cmd(&name, workdir.as_deref());
    run.args(["bash", "-lc", r#"umask 002 && exec "$@""#, "bash"]).args(command);
    let status = docker::run_status(run)?;
    Ok(status.code().unwrap_or(128 + 1))
}

pub fn dir() -> Result<i32> {
    println!("{}", paths::config_dir()?.display());
    Ok(0)
}

/// Kept by dot-claude-clean: credentials, config and skills.
const KEEP: &[&str] = &[".credentials.json", ".claude.json", "settings.json", "skills", ".gitkeep"];
/// Archived before the wipe.
const HISTORY: &[&str] =
    &["file-history", "history.jsonl", "plans", "projects", "session-env", "sessions", "shell-snapshots"];

pub fn dot_claude_clean(dot_claude: Option<&Path>, yes: bool) -> Result<i32> {
    let dir = match dot_claude {
        Some(d) => paths::absolute(d)?,
        None => paths::default_dot_claude()?,
    };
    if !dir.is_dir() {
        bail!("{} is not a directory", paths::display(&dir));
    }
    let parent = dir.parent().context("dot-claude has no parent directory")?.to_path_buf();
    let base = dir.file_name().context("odd dot-claude path")?.to_string_lossy().into_owned();

    let mut remove: Vec<String> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| !KEEP.contains(&n.as_str()))
        .collect();
    remove.sort();
    let archive_items: Vec<&str> = HISTORY.iter().copied().filter(|n| dir.join(n).exists()).collect();
    let stamp = paths::format_local(std::time::SystemTime::now(), "%Y%m%d-%H%M%S");
    let archive = parent.join("session-history").join(format!("{base}-{stamp}.tar.gz"));

    let users: Vec<String> = docker::dockerclaude_containers()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|n| docker::container(&n).ok().flatten())
        .filter(|c| {
            c.running()
                && c.dot_claude().is_some_and(|d| paths::resolve(&d).ok() == paths::resolve(&dir).ok())
        })
        .map(|c| c.name)
        .collect();

    if remove.is_empty() {
        ui::ok(format!("{} is already clean", paths::display(&dir)));
        return Ok(0);
    }

    Plan::new(format!("Wipe {}", ui::bold(paths::display(&dir))))
        .row("delete", remove.join("\n"))
        .row("keep", KEEP.iter().copied().filter(|k| dir.join(k).exists()).collect::<Vec<_>>().join(", "))
        .row(
            "archive",
            if archive_items.is_empty() {
                "nothing to archive".into()
            } else {
                format!("{} → {}", archive_items.join(", "), paths::display(&archive))
            },
        )
        .note(if users.is_empty() {
            String::new()
        } else {
            format!("Running containers use it: {} (their claude sessions may break)", users.join(", "))
        })
        .confirm(yes)?;

    if !archive_items.is_empty() {
        std::fs::create_dir_all(archive.parent().unwrap())?;
        let mut tar = std::process::Command::new("tar");
        tar.arg("-C").arg(&parent).arg("-czf").arg(&archive);
        tar.args(archive_items.iter().map(|i| format!("{base}/{i}")));
        docker::run(tar).context("archiving the session history failed, nothing was deleted")?;
        ui::ok(format!("saved session history to {}", paths::display(&archive)));
    }
    for n in &remove {
        let p = dir.join(n);
        let res = if p.is_dir() && !p.is_symlink() {
            std::fs::remove_dir_all(&p)
        } else {
            std::fs::remove_file(&p)
        };
        res.with_context(|| format!("removing {}", p.display()))?;
    }
    ui::ok(format!("removed {} entries from {}", remove.len(), paths::display(&dir)));
    Ok(0)
}
