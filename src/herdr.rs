//! Herdr (herdr-app + herdr-core's `hc`) integration.
//!
//! How Herdr models things, as far as dockerclaude cares:
//! - Every terminal is an hc terminal. Herdr panes get `HC_TERMINAL_ID`, and
//!   Herdr labels its terminals `herdr.space=<id>` and `herdr.tab=<id>`.
//! - Space names live in the core's `herdr-app` document (`doc.get`); an
//!   empty name means nobody named the space (Herdr then shows its folder).
//! - Spaces belong to one machine. A container is added as an hc machine
//!   reached with `docker exec`, and claude runs there in a terminal labelled
//!   with a space and tab id, which makes Herdr show it as a pane in a space
//!   on that machine. Agents that claude starts with `hc run` show up under it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::ui;

/// The Herdr pane dockerclaude was started from.
#[derive(Debug, Clone)]
pub struct Pane {
    /// The `herdr.space` label: the id of the space the pane is in.
    pub space_id: String,
    /// The space's name as the user set it; empty if unnamed.
    pub space_name: String,
    /// Where the space is (its first terminal's folder), for messages.
    pub space_folder: Option<String>,
}

pub fn hc_available() -> bool {
    which("hc").is_some()
}

pub fn which(prog: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(prog)).find(|p| {
        use std::os::unix::fs::PermissionsExt;
        p.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    })
}

fn hc_json(args: &[&str]) -> Result<Value> {
    let out =
        Command::new("hc").args(args).arg("--json").stdin(Stdio::null()).output().context("running hc")?;
    if !out.status.success() {
        bail!("`hc {}` failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    serde_json::from_slice(&out.stdout).with_context(|| format!("parsing output of `hc {}`", args.join(" ")))
}

/// One JSON-RPC call over `hc connect` (the API on stdin/stdout).
fn rpc(method: &str, params: Value) -> Result<Value> {
    let mut child = Command::new("hc")
        .arg("connect")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("running hc connect")?;
    let req = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{req}")?;
    stdin.flush()?;
    let stdout = child.stdout.take().unwrap();
    let mut result = None;
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v.get("id") == Some(&json!(1)) {
            result = Some(v);
            break;
        }
    }
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    let v = result.context("hc connect closed without answering")?;
    if let Some(err) = v.get("error") {
        bail!("hc {method}: {err}");
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

/// The labels of terminal `t`.
pub fn terminal_labels(t: &str) -> Result<HashMap<String, String>> {
    let v = hc_json(&["get", t])?;
    Ok(v.get("labels")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect())
        .unwrap_or_default())
}

/// The spaces in the `herdr-app` document of this machine's core, or of
/// machine `on`.
fn spaces(on: Option<&str>) -> Result<Vec<Value>> {
    let mut params = json!({"name": "herdr-app"});
    if let Some(m) = on {
        params["on"] = json!(m);
    }
    let v = rpc("doc.get", params)?;
    Ok(v.pointer("/value/spaces").and_then(Value::as_array).cloned().unwrap_or_default())
}

/// Finds the Herdr pane we are running in, if any. Errors (no hc, a pane of
/// another machine, an unreachable core) are warnings: dockerclaude then
/// simply behaves as outside Herdr.
pub fn detect() -> Option<Pane> {
    let terminal = std::env::var("HC_TERMINAL_ID").ok().filter(|t| !t.is_empty())?;
    if !hc_available() {
        ui::warn("HC_TERMINAL_ID is set but hc is not on PATH; ignoring Herdr");
        return None;
    }
    match detect_in(&terminal) {
        Ok(pane) => pane,
        Err(e) => {
            ui::warn(format!("could not determine the Herdr space ({e:#}); ignoring Herdr"));
            None
        }
    }
}

fn detect_in(terminal: &str) -> Result<Option<Pane>> {
    let labels = terminal_labels(terminal)?;
    let Some(space_id) = labels.get("herdr.space").filter(|s| !s.is_empty()).cloned() else {
        // An hc terminal that Herdr doesn't show (e.g. `hc run` from a shell).
        return Ok(None);
    };
    let space =
        spaces(None)?.into_iter().find(|s| s.get("id").and_then(Value::as_str) == Some(space_id.as_str()));
    let get = |k: &str| space.as_ref().and_then(|s| s.get(k)).and_then(Value::as_str).map(String::from);
    Ok(Some(Pane { space_name: get("name").unwrap_or_default(), space_folder: get("folder"), space_id }))
}

/// Whether machine `name` has been added (`hc machine ls`).
pub fn machine_exists(name: &str) -> Result<bool> {
    let v = hc_json(&["machine", "ls"])?;
    Ok(v.get("machines")
        .and_then(Value::as_array)
        .map(|ms| ms.iter().any(|m| m.get("name").and_then(Value::as_str) == Some(name)))
        .unwrap_or(false))
}

/// The `--via` command for reaching container `container`: hc appends the
/// shell line to run there. The host's core runs it with its own
/// environment, so pin down docker's path and DOCKER_HOST as seen here.
pub fn via_command(container: &str) -> String {
    let docker = which("docker").map(|p| p.display().to_string()).unwrap_or_else(|| "docker".into());
    let mut via = String::new();
    if let Ok(host) = std::env::var("DOCKER_HOST")
        && !host.is_empty()
    {
        via.push_str(&format!("env DOCKER_HOST={} ", ui::shell_quote(&host)));
    }
    via.push_str(&format!("{} exec -i {container} sh -c", ui::shell_quote(&docker)));
    via
}

pub fn machine_add(name: &str, container: &str) -> Result<()> {
    let via = via_command(container);
    let args = ["machine".to_string(), "add".into(), "--via".into(), via, "--name".into(), name.into()];
    ui::cmdline(&std::iter::once("hc".to_string()).chain(args.iter().cloned()).collect::<Vec<_>>());
    let status = Command::new("hc").args(&args).status().context("running hc")?;
    if !status.success() {
        bail!("hc machine add failed");
    }
    Ok(())
}

pub fn machine_rm(name: &str) -> Result<()> {
    hc_json(&["machine", "rm", name]).map(|_| ())
}

/// A fresh tab id, in Herdr's style (`tb` + 32 hex digits).
pub fn new_tab_id() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut bytes);
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("tb{hex}")
}

/// Starts `argv` on `machine` in `cwd`, in a new tab of space `space_id`
/// there, and returns its address (`machine/t3`). It has no parent, so it
/// keeps running when the pane that started it closes.
pub fn run_in_space(machine: &str, cwd: &str, space_id: &str, tab_id: &str, argv: &[&str]) -> Result<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "--bg".into(),
        "--json".into(),
        "--no-parent".into(),
        "--on".into(),
        machine.into(),
        "--cwd".into(),
        cwd.into(),
        "--label".into(),
        format!("herdr.space={space_id}"),
        "--label".into(),
        format!("herdr.tab={tab_id}"),
        "--".into(),
    ];
    args.extend(argv.iter().map(|s| s.to_string()));
    ui::cmdline(&std::iter::once("hc".to_string()).chain(args.iter().cloned()).collect::<Vec<_>>());
    let out = Command::new("hc").args(&args).stdin(Stdio::null()).output().context("running hc run")?;
    if !out.status.success() {
        bail!("hc run failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let v: Value = serde_json::from_slice(&out.stdout).context("parsing hc run output")?;
    Ok(v.get("address").and_then(Value::as_str).unwrap_or("?").to_string())
}

/// Names the space `space_id` on `machine` `name`, unless someone already
/// named it. Needs the Herdr window (herdr-app); best effort, since the
/// window may take a moment to pick up a space it has not seen before.
pub fn name_space(machine: &str, space_id: &str, name: &str) {
    if which("herdr-app").is_none() {
        return;
    }
    let named = spaces(Some(machine)).ok().and_then(|ss| {
        ss.iter()
            .find(|s| s.get("id").and_then(Value::as_str) == Some(space_id))
            .and_then(|s| s.get("name").and_then(Value::as_str).map(|n| !n.is_empty()))
    });
    if named == Some(true) {
        return;
    }
    let args = json!({"id": format!("{machine}/{space_id}"), "name": name}).to_string();
    for _ in 0..10 {
        let Ok(out) =
            Command::new("herdr-app").args(["do", "space.rename", &args]).stdin(Stdio::null()).output()
        else {
            return;
        };
        // Done, or no window to ask (it fails at once then; no point retrying).
        if out.status.success() || String::from_utf8_lossy(&out.stderr).contains("no Herdr window") {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}
