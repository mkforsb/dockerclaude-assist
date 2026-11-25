//! Thin wrapper around the `docker` CLI.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// Container label holding the dockerclaude space name.
pub const LABEL_SPACE: &str = "dockerclaude.space";
/// Container (and, via `docker commit`, snapshot image) label holding the
/// host path bind-mounted at /workspace/.claude.
pub const LABEL_DOT_CLAUDE: &str = "dockerclaude.dot-claude";
pub const IMAGE_REPO: &str = "dockerclaude";
pub const CONTAINER_PREFIX: &str = "dockerclaude-";

pub fn image_ref(tag: &str) -> String {
    format!("{IMAGE_REPO}:{tag}")
}

pub fn cmd<I, S>(args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut c = Command::new("docker");
    c.args(args);
    c
}

fn describe(c: &Command) -> String {
    std::iter::once(c.get_program())
        .chain(c.get_args())
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Runs with inherited stdio; fails on a nonzero exit.
pub fn run(mut c: Command) -> Result<()> {
    let status = c.status().with_context(|| format!("running {}", describe(&c)))?;
    if !status.success() {
        bail!("`{}` failed ({})", describe(&c), exit_text(status));
    }
    Ok(())
}

/// Runs with inherited stdio and returns the exit status as is.
pub fn run_status(mut c: Command) -> Result<ExitStatus> {
    c.status().with_context(|| format!("running {}", describe(&c)))
}

/// Captures stdout; fails with stderr on a nonzero exit.
pub fn output(mut c: Command) -> Result<String> {
    let out = c.stdin(Stdio::null()).output().with_context(|| format!("running {}", describe(&c)))?;
    if !out.status.success() {
        bail!("`{}` failed: {}", describe(&c), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn exit_text(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit code {code}"),
        None => "killed by a signal".into(),
    }
}

#[derive(Debug, Clone)]
pub struct Container {
    pub name: String,
    /// The image reference it was created from, e.g. `dockerclaude:latest`.
    pub image: String,
    pub image_id: String,
    /// `created`, `running`, `paused`, `restarting`, `exited`, ...
    pub status: String,
    pub pid: u32,
    pub labels: HashMap<String, String>,
    /// Bind mounts as (host source, container destination).
    pub binds: Vec<(PathBuf, PathBuf)>,
    /// The networks it is connected to (`bridge` for the default one).
    pub networks: Vec<String>,
}

impl Container {
    pub fn running(&self) -> bool {
        self.status == "running"
    }

    /// The tag of the image it was created from (`latest` for `dockerclaude:latest`).
    pub fn image_tag(&self) -> Option<&str> {
        self.image.strip_prefix(IMAGE_REPO)?.strip_prefix(':')
    }

    /// The host directory mounted at /workspace/.claude.
    pub fn dot_claude(&self) -> Option<PathBuf> {
        self.binds
            .iter()
            .find(|(_, dst)| dst == &PathBuf::from("/workspace/.claude"))
            .map(|(src, _)| src.clone())
            .or_else(|| self.labels.get(LABEL_DOT_CLAUDE).map(PathBuf::from))
    }

    fn from_json(v: &Value) -> Self {
        let s = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or("").to_string();
        let labels = v
            .pointer("/Config/Labels")
            .and_then(Value::as_object)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect())
            .unwrap_or_default();
        let binds = v
            .pointer("/Mounts")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter(|m| m.get("Type").and_then(Value::as_str) == Some("bind"))
                    .map(|m| {
                        let p = |k: &str| PathBuf::from(m.get(k).and_then(Value::as_str).unwrap_or(""));
                        (p("Source"), p("Destination"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut networks: Vec<String> = v
            .pointer("/NetworkSettings/Networks")
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        networks.sort();
        Self {
            name: s("/Name").trim_start_matches('/').to_string(),
            image: s("/Config/Image"),
            image_id: s("/Image"),
            status: s("/State/Status"),
            pid: v.pointer("/State/Pid").and_then(Value::as_u64).unwrap_or(0) as u32,
            labels,
            binds,
            networks,
        }
    }
}

/// Inspects container `name`; `None` if there is no such container.
pub fn container(name: &str) -> Result<Option<Container>> {
    let out = cmd(["container", "inspect", name])
        .stdin(Stdio::null())
        .output()
        .context("running docker (is it installed?)")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("No such container") || err.contains("no such container") {
            return Ok(None);
        }
        bail!("docker container inspect {name}: {}", err.trim());
    }
    let v: Value = serde_json::from_slice(&out.stdout).context("parsing docker inspect output")?;
    Ok(v.as_array().and_then(|a| a.first()).map(Container::from_json))
}

/// The names of all containers (running or not) named `dockerclaude-*`.
pub fn dockerclaude_containers() -> Result<Vec<String>> {
    let out = output(cmd([
        "ps",
        "-a",
        "--filter",
        &format!("name=^{CONTAINER_PREFIX}"),
        "--format",
        "{{.Names}}",
    ]))?;
    let mut names: Vec<String> =
        out.lines().map(str::trim).filter(|n| n.starts_with(CONTAINER_PREFIX)).map(String::from).collect();
    names.sort();
    Ok(names)
}

/// Whether there is a network called `name`.
pub fn network_exists(name: &str) -> Result<bool> {
    let out = output(cmd(["network", "ls", "--filter", &format!("name=^{name}$"), "--format", "{{.Name}}"]))?;
    Ok(out.lines().any(|l| l.trim() == name))
}

/// Creates network `name` (a bridge), labelled with its space. Succeeds if it
/// appeared meanwhile (e.g. created by a concurrent dockerclaude).
pub fn network_create(name: &str, space: &str) -> Result<()> {
    let res = output(cmd(["network", "create", "--label", &format!("{LABEL_SPACE}={space}"), name]));
    match res {
        Err(_) if network_exists(name)? => Ok(()),
        r => r.map(|_| ()),
    }
}

/// The names of all containers, running or not, connected to network `name`.
pub fn network_containers(name: &str) -> Result<Vec<String>> {
    let out = output(cmd(["ps", "-a", "--filter", &format!("network={name}"), "--format", "{{.Names}}"]))?;
    Ok(out.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
}

#[derive(Debug, Clone)]
pub struct Image {
    pub id: String,
    pub labels: HashMap<String, String>,
}

impl Image {
    pub fn short_id(&self) -> &str {
        short_id(&self.id)
    }
}

pub fn short_id(id: &str) -> &str {
    let id = id.strip_prefix("sha256:").unwrap_or(id);
    &id[..id.len().min(12)]
}

/// Inspects image `reference`; `None` if there is no such image.
pub fn image(reference: &str) -> Result<Option<Image>> {
    let out = cmd(["image", "inspect", reference])
        .stdin(Stdio::null())
        .output()
        .context("running docker (is it installed?)")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.to_ascii_lowercase().contains("no such image") {
            return Ok(None);
        }
        bail!("docker image inspect {reference}: {}", err.trim());
    }
    let v: Value = serde_json::from_slice(&out.stdout).context("parsing docker inspect output")?;
    let Some(v) = v.as_array().and_then(|a| a.first()) else { return Ok(None) };
    let labels = v
        .pointer("/Config/Labels")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect())
        .unwrap_or_default();
    Ok(Some(Image { id: v.get("Id").and_then(Value::as_str).unwrap_or("").to_string(), labels }))
}

/// Names of the containers (running or not) created from image `id`.
pub fn containers_using_image(id: &str) -> Result<Vec<String>> {
    let out = output(cmd(["ps", "-a", "--filter", &format!("ancestor={id}"), "--format", "{{.Names}}"]))?;
    Ok(out.lines().map(String::from).collect())
}

/// `Up 3 hours`, `Exited (0) 2 days ago`, ... as `docker ps` shows it.
pub fn ps_status(name: &str) -> String {
    output(cmd(["ps", "-a", "--filter", &format!("name=^{name}$"), "--format", "{{.Status}}"]))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// How long ago image `reference` was created, as `docker images` shows it.
pub fn image_created_since(reference: &str) -> String {
    output(cmd(["image", "ls", "--format", "{{.CreatedSince}}", reference]))
        .map(|s| s.lines().next().unwrap_or("").trim().to_string())
        .unwrap_or_default()
}

pub fn root_dir() -> Result<PathBuf> {
    Ok(PathBuf::from(output(cmd(["info", "-f", "{{.DockerRootDir}}"]))?.trim()))
}

/// The last `n` lines of the container's log (stdout and stderr).
pub fn log_tail(name: &str, n: usize) -> Vec<String> {
    let out = cmd(["logs", "--tail", &n.to_string(), name]).stdin(Stdio::null()).output();
    let Ok(out) = out else { return Vec::new() };
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let lines: Vec<String> = text.lines().map(String::from).collect();
    let skip = lines.len().saturating_sub(n);
    lines.into_iter().skip(skip).collect()
}

/// Environment variables forwarded from the calling terminal, both when a
/// container is created and on every `docker exec` (a container created
/// without a terminal would otherwise lack colors and Shift-Enter in claude).
pub const TERMINAL_ENV: &[&str] = &["TERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION", "COLORTERM", "TMUX"];

/// `-e VAR` for each terminal variable set here (unset ones are not passed).
pub fn terminal_env_args() -> Vec<String> {
    TERMINAL_ENV
        .iter()
        .filter(|v| std::env::var_os(v).is_some())
        .flat_map(|v| ["-e".to_string(), v.to_string()])
        .collect()
}
