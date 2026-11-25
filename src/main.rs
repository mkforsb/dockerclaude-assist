//! dockerclaude: Claude Code in Docker containers, one per space, with
//! host directories mounted in on demand and optional Herdr integration.

mod cli;
mod cmd;
mod ctrbind;
mod docker;
mod herdr;
mod mounts;
mod paths;
mod space;
mod tmpfs;
mod ui;

use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Cmd, SpaceArg};
use space::Unnamed;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(e) if e.downcast_ref::<ui::Declined>().is_some() => {
            eprintln!("Cancelled, nothing was changed.");
            ExitCode::from(1)
        }
        Err(e) => {
            ui::error(format!("{e:#}"));
            ExitCode::from(1)
        }
    }
}

/// The space for a subcommand: --space, else the named Herdr space, else "default".
fn space_of(arg: SpaceArg) -> Result<String> {
    Ok(space::resolve(arg.space, Unnamed::Note)?.space)
}

fn run(cli: Cli) -> Result<i32> {
    use cmd::*;
    let Some(command) = cli.command else {
        let s = cli.session;
        return session::session(s.space.space, s.streaming_json, s.yes.yes);
    };
    let done = |r: Result<()>| r.map(|()| 0);
    match command {
        Cmd::BuildImage { tag, yes } => done(container::build_image(tag.image_tag.as_deref(), yes.yes)),
        Cmd::StartContainer { space, tag, dot_claude, tmpfs, yes } => done(container::start_container(
            &space_of(space)?,
            tag.image_tag.as_deref(),
            dot_claude.dot_claude.as_deref(),
            tmpfs.tmpfs.as_deref(),
            yes.yes,
        )),
        Cmd::StopContainer { space, yes } => done(container::stop_container(&space_of(space)?, yes.yes)),
        Cmd::RmContainer { space, yes } => done(container::rm_container(&space_of(space)?, yes.yes)),
        Cmd::Install { space, yes, names } => misc::install(&space_of(space)?, &names, yes.yes),
        Cmd::Exec { space, yes, command } => misc::exec(&space_of(space)?, &command, yes.yes),
        Cmd::Dir => misc::dir(),
        Cmd::DotClaudeClean { dot_claude, yes } => {
            misc::dot_claude_clean(dot_claude.dot_claude.as_deref(), yes.yes)
        }
        Cmd::Snapshot { space, yes } => done(snapshot::snapshot(&space_of(space)?, yes.yes)),
        Cmd::SnapshotDelete { space, yes } => done(snapshot::snapshot_delete(&space_of(space)?, yes.yes)),
        Cmd::Restore { space, tmpfs, yes } => {
            done(snapshot::restore(&space_of(space)?, tmpfs.tmpfs.as_deref(), yes.yes))
        }
        Cmd::Mount { space, yes, ro, path } => done(mount::mount(&space_of(space)?, &path, ro, yes.yes)),
        Cmd::Umount { space, yes, path } => done(mount::umount(&space_of(space)?, &path, yes.yes)),
        Cmd::Ps { space, all } => {
            let space = if all { None } else { Some(space_of(space)?) };
            ps::ps(space.as_deref())
        }
        Cmd::BashCompletion => completion::bash_completion(),
        Cmd::List { what, space } => completion::list(&what, space),
        Cmd::CtrBind { action, pid, ro, paths } => ctr_bind(&action, pid, ro, &paths),
    }
}

/// The hidden mount helper (see ctrbind.rs): runs in its own process since it
/// joins the container's namespaces for good.
fn ctr_bind(action: &str, pid: u32, ro: bool, paths: &[String]) -> Result<i32> {
    let res = match (action, paths) {
        ("mount", [src, dst]) => ctrbind::bind(pid, src, dst, ro),
        ("umount", [dst]) => ctrbind::unbind(pid, dst, mounts::ROOT),
        _ => anyhow::bail!("usage: __ctr-bind mount --pid PID [--ro] SRC DST | umount --pid PID DST"),
    };
    res.map(|()| 0).map_err(Into::into)
}
