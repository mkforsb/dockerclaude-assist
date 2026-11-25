//! `dockerclaude` without a subcommand: a Claude Code session in the
//! current directory.

use anyhow::{Context, Result};

use super::container::{build_image, start_container};
use super::misc::exec_cmd;
use crate::docker;
use crate::herdr::{self, Pane};
use crate::mounts::{self, Visibility};
use crate::space::{self, Target, Unnamed};
use crate::ui::{self, Plan};

const STREAMING_ARGS: &[&str] = &[
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
    "--include-partial-messages",
    "--permission-prompt-tool",
    "stdio",
    "--permission-mode",
    "auto",
    "--disallowedTools",
    "AskUserQuestion",
];

pub fn session(explicit_space: Option<String>, streaming_json: bool, yes: bool) -> Result<i32> {
    let target = space::resolve(explicit_space, Unnamed::Confirm { yes })?;
    let Target { space, pane } = &target;
    let name = target.container();

    // 1. + 2. The image and the container.
    let mut container = docker::container(&name)?;
    if container.is_none() && docker::image(&docker::image_ref("latest"))?.is_none() {
        build_image(None, yes)?;
    }
    if !container.as_ref().is_some_and(|c| c.running()) {
        start_container(space, None, None, None, yes)?;
        container = docker::container(&name)?;
    }
    let c = container.context("the container is missing after starting it")?;

    // 3. The current directory.
    let cwd = std::env::current_dir()?.canonicalize().context("resolving the current directory")?;
    let current = mounts::list(&name, c.pid)?;
    match mounts::visibility(&cwd, &current) {
        Visibility::Mounted(_) | Visibility::Inside(_) => {}
        _ => super::mount::mount(space, &cwd, false, yes)?,
    }
    let workdir = mounts::target_for(&cwd);

    // 4. Claude Code.
    if streaming_json {
        // For programs talking stream-json over stdin/stdout: never via Herdr.
        let mut run = docker::cmd(["exec", "-i", "-w"]);
        run.arg(&workdir).args(docker::terminal_env_args()).arg(&name);
        run.args(["bash", "-lc", r#"umask 002 && exec claude "$@""#, "bash"]).args(STREAMING_ARGS);
        return Ok(docker::run_status(run)?.code().unwrap_or(1));
    }
    match pane {
        Some(p) => in_herdr(space, &name, p, &workdir.to_string_lossy(), yes),
        None => {
            ui::step(format!("claude in {name}:{}", workdir.display()));
            let mut run = exec_cmd(&name, Some(&workdir));
            run.args(["bash", "-lc", "umask 002 && exec claude"]);
            Ok(docker::run_status(run)?.code().unwrap_or(1))
        }
    }
}

/// Runs claude through Herdr: in a new tab of a space on the container's
/// machine (`dockerclaude-<space>`), using the id of the Herdr space we run
/// in, so each Herdr space gets its own space on the machine.
fn in_herdr(space: &str, machine: &str, pane: &Pane, workdir: &str, yes: bool) -> Result<i32> {
    if !herdr::machine_exists(machine)? {
        Plan::new(format!("Add container {} to Herdr as a machine", ui::bold(machine)))
            .row("machine", machine)
            .row("reached with", herdr::via_command(machine))
            .note("Herdr then shows the container's spaces and agents beside this computer's")
            .note("hc may offer to copy this computer's hc into the container if its own is older")
            .confirm(yes)?;
        herdr::machine_add(machine, machine)?;
        ui::ok(format!("added machine {machine}"));
    }

    let tab = herdr::new_tab_id();
    ui::step(format!("claude in {machine}:{workdir}, via Herdr"));
    let address = herdr::run_in_space(machine, workdir, &pane.space_id, &tab, &["claude"])?;
    herdr::name_space(machine, &pane.space_id, space);
    ui::ok(format!("started {address} in a new tab of space `{space}` on machine {machine} (Herdr sidebar)"));
    ui::note(format!("attach here instead with: hc attach {address}"));
    Ok(0)
}
