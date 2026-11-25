//! Host directories mounted into a container under /mnt, at their full host
//! path (`/home/u/foo` → `/mnt/home/u/foo`). The container's mount table is
//! the only state: nothing is recorded anywhere else.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, bail};

use crate::ctrbind;
use crate::docker;

pub const ROOT: &str = "/mnt";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// The host path, i.e. the container path without `/mnt`.
    pub host: PathBuf,
    /// Where it is in the container.
    pub target: PathBuf,
    pub readonly: bool,
}

/// `/mnt` + the absolute host path.
pub fn target_for(host: &Path) -> PathBuf {
    let rel = host.strip_prefix("/").unwrap_or(host);
    Path::new(ROOT).join(rel)
}

/// Undoes the octal escapes of /proc/*/mountinfo (`\040` for a space, ...).
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            let v = (b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0');
            out.push(v);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The mounts below /mnt in a mountinfo table, the last one winning when
/// several are stacked on the same mount point.
pub fn parse_mountinfo(text: &str) -> Vec<Mount> {
    let mut out: Vec<Mount> = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        if f.len() < 6 {
            continue;
        }
        let target = PathBuf::from(unescape(f[4]));
        let Ok(rel) = target.strip_prefix(ROOT) else { continue };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let readonly = f[5].split(',').any(|o| o == "ro");
        let m = Mount { host: Path::new("/").join(rel), target, readonly };
        out.retain(|o| o.target != m.target);
        out.push(m);
    }
    out.sort_by(|a, b| a.host.cmp(&b.host));
    out
}

/// The mounts below /mnt in the running container `name` (process `pid`).
pub fn list(name: &str, pid: u32) -> Result<Vec<Mount>> {
    // Readable from the host for our own (rootless) containers; otherwise
    // ask the container itself.
    let text = match std::fs::read_to_string(format!("/proc/{pid}/mountinfo")) {
        Ok(t) => t,
        Err(_) => docker::output({
            let mut c = docker::cmd(["exec", name, "cat", "/proc/1/mountinfo"]);
            c.stdin(Stdio::null());
            c
        })?,
    };
    Ok(parse_mountinfo(&text))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Visibility {
    /// Mounted at exactly this path.
    Mounted(Mount),
    /// Visible through a mount of a parent directory.
    Inside(Mount),
    /// Not visible; mounting it is fine.
    Absent,
    /// Not visible, but directories inside it are mounted, so mounting it
    /// would stack on top of (and hide) those.
    Covers(Vec<Mount>),
}

pub fn visibility(host: &Path, mounts: &[Mount]) -> Visibility {
    if let Some(m) = mounts.iter().find(|m| m.host == host) {
        return Visibility::Mounted(m.clone());
    }
    if let Some(m) = mounts.iter().find(|m| host.starts_with(&m.host)) {
        return Visibility::Inside(m.clone());
    }
    let nested: Vec<Mount> = mounts.iter().filter(|m| m.host.starts_with(host)).cloned().collect();
    if nested.is_empty() { Visibility::Absent } else { Visibility::Covers(nested) }
}

/// Bind-mounts `host` (absolute, resolved) into the container of `pid`.
pub fn mount(pid: u32, host: &Path, readonly: bool) -> Result<()> {
    let src = host.to_str().context("the path is not valid UTF-8")?;
    let dst = target_for(host);
    let mut c = ctrbind::helper_command()?;
    c.args(["__ctr-bind", "mount", "--pid", &pid.to_string()]);
    if readonly {
        c.arg("--ro");
    }
    c.arg(src).arg(&dst);
    run_helper(c)
}

/// Unmounts `m` from the container of `pid` and removes its now empty
/// mount point directories below /mnt.
pub fn umount(pid: u32, m: &Mount) -> Result<()> {
    let mut c = ctrbind::helper_command()?;
    c.args(["__ctr-bind", "umount", "--pid", &pid.to_string()]).arg(&m.target);
    run_helper(c)
}

fn run_helper(mut c: std::process::Command) -> Result<()> {
    let out = c.stdin(Stdio::null()).output().context("running the mount helper")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("error: "));
    }
    Ok(())
}

/// Processes in container `name` whose working directory is `dir` or inside
/// it, as `pid command` lines (best effort).
pub fn processes_in(name: &str, dir: &Path) -> Vec<String> {
    let script = r#"for p in /proc/[0-9]*; do
  c=$(readlink "$p/cwd" 2>/dev/null) || continue
  case "$c" in "$1"|"$1"/*) printf '%s %s\n' "${p#/proc/}" "$(tr '\0' ' ' < "$p/cmdline" | cut -c1-70)";; esac
done"#;
    let Some(d) = dir.to_str() else { return Vec::new() };
    let out = docker::output({
        let mut c = docker::cmd(["exec", name, "sh", "-c", script, "sh", d]);
        c.stdin(Stdio::null());
        c
    });
    out.map(|s| s.lines().map(String::from).filter(|l| !l.trim().is_empty()).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = "\
600 500 0:50 / / rw,relatime - overlay overlay rw
601 600 0:51 / /proc rw - proc proc rw
602 600 8:1 /user/.config/dockerclaude/dot-claude /workspace/.claude rw - ext4 /dev/sda1 rw
610 600 8:1 /user/Desktop/foo /mnt/home/user/Desktop/foo rw,relatime - ext4 /dev/sda1 rw
611 600 8:1 /user/My\\040Docs /mnt/home/user/My\\040Docs ro,relatime - ext4 /dev/sda1 rw
612 600 8:1 /user/Desktop/foo /mnt/home/user/Desktop/foo rw,relatime - ext4 /dev/sda1 rw
";

    #[test]
    fn parses_mountinfo() {
        let ms = parse_mountinfo(INFO);
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0].host, PathBuf::from("/home/user/Desktop/foo"));
        assert!(!ms[0].readonly);
        assert_eq!(ms[1].host, PathBuf::from("/home/user/My Docs"));
        assert_eq!(ms[1].target, PathBuf::from("/mnt/home/user/My Docs"));
        assert!(ms[1].readonly);
    }

    #[test]
    fn visibility_cases() {
        let ms = parse_mountinfo(INFO);
        let p = |s: &str| PathBuf::from(s);
        assert!(matches!(visibility(&p("/home/user/Desktop/foo"), &ms), Visibility::Mounted(_)));
        assert!(matches!(visibility(&p("/home/user/Desktop/foo/src"), &ms), Visibility::Inside(_)));
        assert_eq!(visibility(&p("/home/user/Desktop/foobar"), &ms), Visibility::Absent);
        assert!(matches!(visibility(&p("/home/user"), &ms), Visibility::Covers(v) if v.len() == 2));
    }

    #[test]
    fn targets() {
        assert_eq!(target_for(Path::new("/home/u/x")), PathBuf::from("/mnt/home/u/x"));
    }
}
