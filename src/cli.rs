use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::space;

const AFTER_HELP: &str = "\
Without a subcommand, starts a Claude Code session in the current directory:
builds the image and starts the container if needed, mounts the directory into
the container (at /mnt/<absolute path>) and runs claude there. In a Herdr pane,
claude opens in a new tab of a space on the container's machine instead.

Spaces default to the name of the Herdr space you are in (if it has one), else
\"default\". Files live in ~/.config/dockerclaude (see `dockerclaude dir`).";

#[derive(Parser, Debug)]
#[command(name = "dockerclaude", version, about = "Claude Code in Docker containers, one per space", after_help = AFTER_HELP)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Cmd>,

    #[command(flatten)]
    pub session: SessionArgs,
}

#[derive(Args, Debug)]
pub struct SessionArgs {
    #[command(flatten)]
    pub space: SpaceArg,

    /// Run claude with stream-json input and output on stdin/stdout (for programs, not people)
    #[arg(long)]
    pub streaming_json: bool,

    #[command(flatten)]
    pub yes: YesArg,
}

#[derive(Args, Debug, Clone)]
pub struct SpaceArg {
    /// Space to work on [default: the named Herdr space you are in, else "default"]
    #[arg(short, long, value_name = "SPACE", value_parser = space::validate)]
    pub space: Option<String>,
}

#[derive(Args, Debug, Clone, Copy)]
pub struct YesArg {
    /// Don't ask for confirmation
    #[arg(short, long)]
    pub yes: bool,
}

#[derive(Args, Debug, Clone)]
pub struct TagArg {
    /// Image tag: builds ~/.config/dockerclaude/Dockerfile.<TAG> as dockerclaude:<TAG> [default: Dockerfile as dockerclaude:latest]
    #[arg(short = 't', long, value_name = "TAG", value_parser = validate_tag)]
    pub image_tag: Option<String>,
}

#[derive(Args, Debug, Clone)]
pub struct DotClaudeArg {
    /// Host directory mounted at /workspace/.claude [default: ~/.config/dockerclaude/dot-claude]
    #[arg(short, long, value_name = "PATH")]
    pub dot_claude: Option<PathBuf>,
}

#[derive(Args, Debug, Clone)]
pub struct TmpfsArg {
    /// Put the container's writable layer on a tmpfs of this size (e.g. 4g, 25%); lost on reboot or a docker restart
    #[arg(long, value_name = "SIZE")]
    pub tmpfs: Option<String>,
}

fn validate_tag(s: &str) -> Result<String, String> {
    let ok = !s.is_empty()
        && s.len() <= 128
        && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c));
    if ok { Ok(s.to_string()) } else { Err(format!("invalid image tag `{s}`")) }
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Build the dockerclaude image from ~/.config/dockerclaude, replacing the previous one
    BuildImage {
        #[command(flatten)]
        tag: TagArg,
        #[command(flatten)]
        yes: YesArg,
    },

    /// Create and/or start the container dockerclaude-<space>
    StartContainer {
        #[command(flatten)]
        space: SpaceArg,
        #[command(flatten)]
        tag: TagArg,
        #[command(flatten)]
        dot_claude: DotClaudeArg,
        #[command(flatten)]
        tmpfs: TmpfsArg,
        #[command(flatten)]
        yes: YesArg,
    },

    /// Stop the container, keeping it (and everything installed in it)
    StopContainer {
        #[command(flatten)]
        space: SpaceArg,
        #[command(flatten)]
        yes: YesArg,
    },

    /// Remove the container, stopping it first if needed
    RmContainer {
        #[command(flatten)]
        space: SpaceArg,
        #[command(flatten)]
        yes: YesArg,
    },

    /// Run installers from ~/.config/dockerclaude/installers, or apt install packages
    Install {
        #[command(flatten)]
        space: SpaceArg,
        #[command(flatten)]
        yes: YesArg,
        /// Installer names (installers/<NAME>.sh) and/or apt packages
        #[arg(required = true, value_name = "NAME")]
        names: Vec<String>,
    },

    /// Run a command in the container (umask 002, in the current directory if it is mounted)
    Exec {
        #[command(flatten)]
        space: SpaceArg,
        #[command(flatten)]
        yes: YesArg,
        /// The command and its arguments
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true, value_name = "COMMAND")]
        command: Vec<String>,
    },

    /// Print the dockerclaude directory (~/.config/dockerclaude)
    Dir,

    /// Wipe a dot-claude directory except credentials, config and skills, archiving session history first
    DotClaudeClean {
        #[command(flatten)]
        dot_claude: DotClaudeArg,
        #[command(flatten)]
        yes: YesArg,
    },

    /// Save the container as an image to ~/.config/dockerclaude/snapshots/<space>.tar.zst
    Snapshot {
        #[command(flatten)]
        space: SpaceArg,
        #[command(flatten)]
        yes: YesArg,
    },

    /// Delete the snapshot of a space (file and image)
    SnapshotDelete {
        #[command(flatten)]
        space: SpaceArg,
        #[command(flatten)]
        yes: YesArg,
    },

    /// Create and start the container from the space's snapshot
    Restore {
        #[command(flatten)]
        space: SpaceArg,
        #[command(flatten)]
        tmpfs: TmpfsArg,
        #[command(flatten)]
        yes: YesArg,
    },

    /// Mount a host directory into the running container at /mnt/<absolute host path>
    Mount {
        #[command(flatten)]
        space: SpaceArg,
        #[command(flatten)]
        yes: YesArg,
        /// Mount read-only
        #[arg(long)]
        ro: bool,
        /// Host directory (or file)
        #[arg(value_name = "HOST_PATH")]
        path: PathBuf,
    },

    /// Unmount a host directory mounted with `mount`
    Umount {
        #[command(flatten)]
        space: SpaceArg,
        #[command(flatten)]
        yes: YesArg,
        /// Host directory, as given to `mount`
        #[arg(value_name = "HOST_PATH")]
        path: PathBuf,
    },

    /// Show the container(s): state, image, mounts, recent log and last prompt
    Ps {
        #[command(flatten)]
        space: SpaceArg,
        /// All dockerclaude containers (and snapshots)
        #[arg(short, long, conflicts_with = "space")]
        all: bool,
    },

    /// Print the bash completion script; use with: source <(dockerclaude bash-completion)
    BashCompletion,

    /// Completion helper: print spaces, mounts, installers or tags
    #[command(name = "__list", hide = true)]
    List {
        #[arg(value_parser = ["spaces", "mounts", "installers", "tags"])]
        what: String,
        /// Space, for mounts
        #[arg(short, long)]
        space: Option<String>,
    },

    /// Mount helper, run as a separate process (it changes namespaces)
    #[command(name = "__ctr-bind", hide = true)]
    CtrBind {
        #[arg(value_parser = ["mount", "umount"])]
        action: String,
        #[arg(long)]
        pid: u32,
        #[arg(long)]
        ro: bool,
        /// mount: <host-src> <ctr-dst>; umount: <ctr-dst>
        paths: Vec<String>,
    },
}
