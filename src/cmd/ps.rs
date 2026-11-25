//! ps: a summary of one or all containers.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use crate::docker::{self, CONTAINER_PREFIX, Container, LABEL_SPACE};
use crate::mounts::{self, Mount};
use crate::paths;
use crate::space::container_name;
use crate::ui;
use crate::{herdr, tmpfs};

pub fn ps(space: Option<&str>) -> Result<i32> {
    let names = match space {
        Some(s) => vec![container_name(s)],
        None => docker::dockerclaude_containers()?,
    };
    let machines = herdr_machines();
    let mut first = true;
    for name in &names {
        if !first {
            println!();
        }
        first = false;
        match docker::container(name)? {
            Some(c) => show(&c, machines.as_deref()),
            None => {
                println!("{} {}  {}", ui::dim("○"), ui::bold(name), ui::dim("no such container"));
                let s = name.strip_prefix(CONTAINER_PREFIX).unwrap_or(name);
                snapshot_row(s);
            }
        }
    }

    if space.is_none() {
        // Spaces that only exist as a snapshot.
        for s in snapshot_spaces() {
            if names.contains(&container_name(&s)) {
                continue;
            }
            if !first {
                println!();
            }
            first = false;
            println!(
                "{} {}  {}",
                ui::dim("○"),
                ui::bold(container_name(&s)),
                ui::dim("no container, snapshot only")
            );
            snapshot_row(&s);
        }
        if first {
            println!(
                "No dockerclaude containers. Start one with `dockerclaude` or `dockerclaude start-container`."
            );
        }
    }
    Ok(0)
}

fn herdr_machines() -> Option<Vec<String>> {
    if !herdr::hc_available() {
        return None;
    }
    let out = std::process::Command::new("hc").args(["machine", "ls", "--json"]).output().ok()?;
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    Some(
        v.get("machines")?
            .as_array()?
            .iter()
            .filter_map(|m| m.get("name")?.as_str().map(String::from))
            .collect(),
    )
}

fn row(key: &str, value: impl AsRef<str>) {
    const W: usize = 11;
    let mut lines = value.as_ref().lines();
    let pad = " ".repeat(W.saturating_sub(key.len()));
    println!("  {}{pad} {}", ui::cyan(key), lines.next().unwrap_or(""));
    for l in lines {
        println!("  {} {l}", " ".repeat(W));
    }
}

fn snapshot_spaces() -> Vec<String> {
    let Ok(dir) = paths::config_dir().map(|d| d.join("snapshots")) else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".tar.zst").map(String::from))
        .collect();
    v.sort();
    v
}

fn snapshot_row(space: &str) {
    let Ok(file) = paths::snapshot_file(space) else { return };
    if let Ok(m) = std::fs::metadata(&file) {
        row(
            "snapshot",
            format!(
                "{} ({}, {})",
                paths::display(&file),
                paths::human_size(m.len()),
                m.modified().map(paths::local_time).unwrap_or_default()
            ),
        );
    }
}

fn show(c: &Container, machines: Option<&[String]>) {
    let status = docker::ps_status(&c.name);
    let dot = match c.status.as_str() {
        "running" => ui::green("●"),
        "exited" | "created" => ui::yellow("●"),
        _ => ui::red("●"),
    };
    println!("{dot} {}  {}", ui::bold(&c.name), ui::dim(&status));

    let space = c
        .labels
        .get(LABEL_SPACE)
        .cloned()
        .unwrap_or_else(|| c.name.strip_prefix(CONTAINER_PREFIX).unwrap_or(&c.name).to_string());
    row("space", &space);
    let created = docker::image_created_since(&c.image);
    row(
        "image",
        format!(
            "{} ({}{})",
            c.image,
            docker::short_id(&c.image_id),
            if created.is_empty() { String::new() } else { format!(", created {created}") }
        ),
    );
    let dot_claude = c.dot_claude();
    row("dot-claude", dot_claude.as_deref().map(paths::display).unwrap_or_else(|| "-".into()));
    row(
        "network",
        match c.networks.as_slice() {
            [] => "-".into(),
            [n] if n == "bridge" => {
                format!(
                    "bridge {}",
                    ui::dim("(Docker's default; recreate the container for its own network)")
                )
            }
            ns => ns.join(", "),
        },
    );
    if let Ok(Some(t)) = tmpfs::current(&c.name) {
        row("tmpfs", format!("{} ({} used)", paths::human_size(t.size), paths::human_size(t.used)));
    }
    if let Some(ms) = machines {
        row(
            "herdr",
            if ms.contains(&c.name) {
                format!("machine {}", c.name)
            } else {
                ui::dim("not added as a machine (dockerclaude adds it when run in Herdr)")
            },
        );
    }
    snapshot_row(&space);

    let mounted = if c.running() { mounts::list(&c.name, c.pid).unwrap_or_default() } else { Vec::new() };
    if c.running() {
        row(
            "mounts",
            if mounted.is_empty() {
                ui::dim("none")
            } else {
                mounted
                    .iter()
                    .map(|m| {
                        format!(
                            "{}{}",
                            paths::display(&m.host),
                            if m.readonly { ui::dim(" (read-only)") } else { String::new() }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        );
    }

    let log = docker::log_tail(&c.name, 2);
    if !log.is_empty() {
        row("log", log.iter().map(|l| ui::dim(truncate(l, 100))).collect::<Vec<_>>().join("\n"));
    }

    if let Some(d) = &dot_claude
        && let Some((prompt, project, when)) = last_prompt(d, &mounted)
    {
        row(
            "last prompt",
            format!(
                "{}\n{}",
                truncate(&prompt, 100),
                ui::dim(format!("in {}{}", project, when.map(|w| format!(", {w}")).unwrap_or_default()))
            ),
        );
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

/// The last prompt from `<dot-claude>/history.jsonl`, preferring one made
/// in a directory currently mounted in the container (a dot-claude may be
/// shared by several containers). Returns (prompt, project, how long ago).
fn last_prompt(dot_claude: &Path, mounted: &[Mount]) -> Option<(String, String, Option<String>)> {
    let text = std::fs::read_to_string(dot_claude.join("history.jsonl")).ok()?;
    let entries: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let project = |v: &Value| v.get("project").and_then(Value::as_str).map(PathBuf::from);
    let pick = entries
        .iter()
        .rev()
        .find(|v| project(v).is_some_and(|p| mounted.iter().any(|m| p.starts_with(&m.target))))
        .or_else(|| entries.last())?;
    let prompt =
        pick.get("display").and_then(Value::as_str)?.split_whitespace().collect::<Vec<_>>().join(" ");
    let proj = project(pick)
        .map(|p| match p.strip_prefix(mounts::ROOT) {
            Ok(rel) => paths::display(&Path::new("/").join(rel)),
            Err(_) => p.display().to_string(),
        })
        .unwrap_or_else(|| "?".into());
    let when = pick.get("timestamp").and_then(Value::as_u64).map(ago);
    Some((prompt, proj, when))
}

fn ago(ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86400),
    }
}
