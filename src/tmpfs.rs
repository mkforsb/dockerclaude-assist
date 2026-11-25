//! Moves the writable layer of a (stopped) container onto a tmpfs; from
//! extras/docker-tmpfs.sh.
//!
//! The tmpfs is mounted inside rootless dockerd's mount namespace (no sudo
//! needed, and dockerd sees it), so it is gone after a reboot or a restart
//! of the docker daemon. The container then sees the on-disk copy again, as
//! it was when it was moved. Rootless Docker only. Works with both the
//! overlay2 graph driver and the containerd image store.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::docker;

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })))
}

fn dockerd_pid() -> Result<u32> {
    let pidfile = runtime_dir().join("docker.pid");
    let text = std::fs::read_to_string(&pidfile).with_context(|| {
        format!(
            "rootless docker doesn't seem to be running (no {}); --tmpfs needs rootless docker",
            pidfile.display()
        )
    })?;
    text.trim().parse().with_context(|| format!("bad pid in {}", pidfile.display()))
}

/// A command run in rootless dockerd's user and mount namespace, where we
/// are root and can mount things.
fn in_docker_ns(args: &[&str]) -> Result<Command> {
    let mut c = Command::new("nsenter");
    c.args(["-U", "--preserve-credentials", "-m", "-t", &dockerd_pid()?.to_string()]).args(args);
    Ok(c)
}

fn ns_output(args: &[&str]) -> Result<String> {
    let out = in_docker_ns(args)?.stdin(Stdio::null()).output().context("running nsenter")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The directory holding both the upper and the work directory of the
/// container's writable layer. Overlayfs needs those two on the same
/// filesystem, so this whole directory is what goes onto the tmpfs.
pub fn state_dir(container: &str) -> Result<PathBuf> {
    // With the containerd image store, newer docker versions leave out the
    // GraphDriver key entirely. `index` tolerates that, `.GraphDriver` doesn't.
    let out = docker::output(docker::cmd([
        "inspect",
        "-f",
        r#"{{.Id}} {{with index . "GraphDriver"}}{{.Data.UpperDir}}{{end}}"#,
        container,
    ]))?;
    let mut parts = out.trim().splitn(2, ' ');
    let id = parts.next().unwrap_or("").to_string();
    let mut upper = parts.next().unwrap_or("").trim().to_string();
    if upper == "<no value>" {
        upper.clear();
    }

    // The containerd image store doesn't report the upper dir, but the
    // container's snapshot (keyed by the container ID) knows its mount options.
    if upper.is_empty() {
        let sock = runtime_dir().join("docker/containerd/containerd.sock");
        let sock = sock.to_string_lossy();
        if let Ok(mounts) = ns_output(&[
            "ctr",
            "--address",
            &sock,
            "-n",
            "moby",
            "snapshots",
            "--snapshotter",
            "overlayfs",
            "mounts",
            "/unused",
            &id,
        ]) && let Some(pos) = mounts.find("upperdir=")
        {
            upper =
                mounts[pos + "upperdir=".len()..].split([',', ' ', '\n']).next().unwrap_or("").to_string();
        }
    }

    if upper.is_empty() {
        bail!("can't find the writable layer of container {container}");
    }
    Ok(Path::new(&upper).parent().context("odd upper dir")?.to_path_buf())
}

/// A tmpfs holding a container's writable layer.
#[derive(Debug, Clone, Copy)]
pub struct Tmpfs {
    pub size: u64,
    pub used: u64,
}

/// The tmpfs the container's writable layer is on, if it is on one.
pub fn current(container: &str) -> Result<Option<Tmpfs>> {
    let dir = state_dir(container)?;
    let out =
        ns_output(&["findmnt", "-rnb", "-o", "FSTYPE,SIZE,USED", "--mountpoint", &dir.to_string_lossy()])
            .unwrap_or_default();
    // Several lines if stacked; the last is the visible one.
    let Some(line) = out.lines().last() else { return Ok(None) };
    let f: Vec<&str> = line.split_whitespace().collect();
    if f.first() != Some(&"tmpfs") {
        return Ok(None);
    }
    Ok(Some(Tmpfs {
        size: f.get(1).and_then(|s| s.parse().ok()).unwrap_or(0),
        used: f.get(2).and_then(|s| s.parse().ok()).unwrap_or(0),
    }))
}

const NS_MOUNT: &str = r#"set -eu
dir=$1 size=$2 stage=$3
if mountpoint -q "$dir"; then echo "$dir is already a mount point" >&2; exit 1; fi
mount -t tmpfs -o "size=$size" tmpfs "$stage"
chown --reference="$dir" "$stage"
chmod --reference="$dir" "$stage"
# -a keeps ownership, xattrs (user.overlay.*) and whiteouts (0/0 char devices).
if ! cp -a "$dir/." "$stage/"; then
    umount "$stage"
    echo "copying $dir failed, nothing was changed" >&2
    exit 1
fi
mount --move "$stage" "$dir"
"#;

/// Copies the writable layer of stopped container `container` onto a new
/// tmpfs of `size` (tmpfs syntax: `4g`, `25%`) and mounts it in its place.
pub fn move_to_tmpfs(container: &str, size: &str) -> Result<()> {
    parse_size(size)?;
    let status = docker::output(docker::cmd(["inspect", "-f", "{{.State.Status}}", container]))?;
    let status = status.trim();
    if status != "exited" && status != "created" {
        bail!("container {container} is {status}, it must be stopped first");
    }
    let dir = state_dir(container)?;
    let tmp = docker::root_dir()?.join("tmp");
    let stage = tmp.join(format!("dockerclaude-tmpfs.{}", std::process::id()));
    std::fs::create_dir(&stage).with_context(|| format!("creating {}", stage.display()))?;
    let res =
        in_docker_ns(&["sh", "-c", NS_MOUNT, "sh", &dir.to_string_lossy(), size, &stage.to_string_lossy()])
            .and_then(|mut c| {
                let out = c.stdin(Stdio::null()).output().context("running nsenter")?;
                if !out.status.success() {
                    bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
                }
                Ok(())
            });
    let _ = std::fs::remove_dir(&stage);
    res
}

/// Unmounts the tmpfs from the writable layer of stopped container
/// `container`, going back to the on-disk copy.
pub fn unmount(container: &str) -> Result<()> {
    let dir = state_dir(container)?;
    ns_output(&["umount", &dir.to_string_lossy()]).map(|_| ())
}

/// Parses a tmpfs `size=` value into bytes: a number with an optional
/// k/m/g/t/p/e suffix (binary), or a percentage of RAM.
pub fn parse_size(s: &str) -> Result<u64> {
    let s = s.trim();
    let bad = || anyhow::anyhow!("invalid tmpfs size `{s}` (examples: 4g, 512m, 25%)");
    if let Some(pct) = s.strip_suffix('%') {
        let pct: u64 = pct.parse().map_err(|_| bad())?;
        if pct == 0 {
            return Err(bad());
        }
        return Ok(mem_total()? * pct / 100);
    }
    let (num, mult) = match s.chars().last() {
        Some(c) if c.is_ascii_alphabetic() => {
            let mult = match c.to_ascii_lowercase() {
                'k' => 1u64 << 10,
                'm' => 1 << 20,
                'g' => 1 << 30,
                't' => 1 << 40,
                'p' => 1 << 50,
                'e' => 1 << 60,
                _ => return Err(bad()),
            };
            (&s[..s.len() - 1], mult)
        }
        _ => (s, 1),
    };
    let n: u64 = num.parse().map_err(|_| bad())?;
    if n == 0 {
        return Err(bad());
    }
    n.checked_mul(mult).ok_or_else(bad)
}

fn mem_total() -> Result<u64> {
    let info = std::fs::read_to_string("/proc/meminfo")?;
    let kb: u64 = info
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
        .context("no MemTotal in /proc/meminfo")?;
    Ok(kb * 1024)
}

/// Whether a tmpfs of `actual` bytes is what `requested` asks for, allowing
/// for page rounding (and, with percentages, RAM accounting differences).
pub fn size_matches(requested: &str, actual: u64) -> Result<bool> {
    let want = parse_size(requested)?;
    let slack = if requested.trim().ends_with('%') { want / 50 } else { 0 } + 4096;
    Ok(want.abs_diff(actual) <= slack)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("4g").unwrap(), 4 << 30);
        assert_eq!(parse_size("512M").unwrap(), 512 << 20);
        assert_eq!(parse_size("1000").unwrap(), 1000);
        assert!(parse_size("0").is_err());
        assert!(parse_size("4x").is_err());
        assert!(parse_size("g").is_err());
        assert!(parse_size("50%").unwrap() > 0);
        assert!(size_matches("4g", 4 << 30).unwrap());
        assert!(!size_matches("4g", 2 << 30).unwrap());
    }
}
