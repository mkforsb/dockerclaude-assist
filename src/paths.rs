//! Where things live: `~/.config/dockerclaude` and the default files the
//! binary carries for it.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

pub fn home() -> Result<PathBuf> {
    match std::env::var_os("HOME") {
        Some(h) if !h.is_empty() => Ok(PathBuf::from(h)),
        _ => bail!("HOME is not set"),
    }
}

/// `$HOME/.config/dockerclaude`: the Docker build context, installers,
/// the default dot-claude and snapshots.
pub fn config_dir() -> Result<PathBuf> {
    Ok(home()?.join(".config/dockerclaude"))
}

pub fn default_dot_claude() -> Result<PathBuf> {
    Ok(config_dir()?.join("dot-claude"))
}

pub fn installers_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("installers"))
}

pub fn snapshot_file(space: &str) -> Result<PathBuf> {
    Ok(config_dir()?.join("snapshots").join(format!("{space}.tar.zst")))
}

/// `Dockerfile` for the `latest` tag, `Dockerfile.<tag>` for any other.
pub fn dockerfile(tag: &str) -> Result<PathBuf> {
    let name = if tag == "latest" { "Dockerfile".to_string() } else { format!("Dockerfile.{tag}") };
    Ok(config_dir()?.join(name))
}

/// Turns a user-supplied path into an absolute one: expands a leading `~`,
/// resolves it against the current directory and removes `.` and `..`
/// lexically (it need not exist).
pub fn absolute(p: &Path) -> Result<PathBuf> {
    let p = match p.strip_prefix("~") {
        Ok(rest) => home()?.join(rest),
        Err(_) => p.to_path_buf(),
    };
    let p = if p.is_absolute() { p } else { std::env::current_dir()?.join(p) };
    let mut out = PathBuf::from("/");
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(n) => out.push(n),
            _ => {}
        }
    }
    Ok(out)
}

/// Like [`absolute`], but resolves symlinks when the path exists.
pub fn resolve(p: &Path) -> Result<PathBuf> {
    let abs = absolute(p)?;
    Ok(fs::canonicalize(&abs).unwrap_or(abs))
}

pub fn display(p: &Path) -> String {
    if let Ok(home) = home()
        && let Ok(rest) = p.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) };
    }
    p.display().to_string()
}

/// A default file for `~/.config/dockerclaude`.
pub struct Asset {
    pub rel: &'static str,
    pub content: &'static str,
    pub executable: bool,
}

pub const DOCKERFILE: Asset =
    Asset { rel: "Dockerfile", content: include_str!("../assets/Dockerfile"), executable: false };
pub const ENTRYPOINT: Asset =
    Asset { rel: "entrypoint.sh", content: include_str!("../assets/entrypoint.sh"), executable: true };
pub const RUST_INSTALLER: Asset = Asset {
    rel: "installers/rust.sh",
    content: include_str!("../assets/installers/rust.sh"),
    executable: true,
};

/// The default files that `build-image` would write for `tag`: missing ones
/// only, never replacing what is there. Installers are only seeded when the
/// installers directory does not exist at all, so deleting one sticks.
pub fn missing_assets(tag: &str) -> Result<Vec<&'static Asset>> {
    let dir = config_dir()?;
    let mut out = Vec::new();
    if tag == "latest" && !dir.join(DOCKERFILE.rel).exists() {
        out.push(&DOCKERFILE);
    }
    if !dir.join(ENTRYPOINT.rel).exists() {
        out.push(&ENTRYPOINT);
    }
    if !dir.join("installers").exists() {
        out.push(&RUST_INSTALLER);
    }
    Ok(out)
}

pub fn write_assets(assets: &[&Asset]) -> Result<()> {
    let dir = config_dir()?;
    for a in assets {
        let path = dir.join(a.rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&path, a.content).with_context(|| format!("writing {}", path.display()))?;
        if a.executable {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}

/// Tags with a Dockerfile: `latest` for `Dockerfile`, `<tag>` for `Dockerfile.<tag>`.
pub fn dockerfile_tags() -> Vec<String> {
    let Ok(dir) = config_dir() else { return Vec::new() };
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut tags: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| {
            if n == "Dockerfile" {
                Some("latest".to_string())
            } else {
                n.strip_prefix("Dockerfile.").map(String::from)
            }
        })
        .collect();
    tags.sort();
    tags
}

/// Installer names (`rust` for `installers/rust.sh`).
pub fn installer_names() -> Vec<String> {
    let Ok(dir) = installers_dir() else { return Vec::new() };
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".sh").map(String::from))
        .collect();
    names.sort();
    names
}

/// Human-readable size, e.g. `1.4G`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes}B")
    } else if v < 10.0 {
        format!("{v:.1}{}", UNITS[u])
    } else {
        format!("{v:.0}{}", UNITS[u])
    }
}

/// `2026-11-22 21:03` in local time, for a file's modification time.
pub fn local_time(t: std::time::SystemTime) -> String {
    format_local(t, "%Y-%m-%d %H:%M")
}

/// strftime in local time.
pub fn format_local(t: std::time::SystemTime, fmt: &str) -> String {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let mut buf = [0u8; 64];
    let cfmt = std::ffi::CString::new(fmt).unwrap();
    unsafe {
        libc::localtime_r(&secs, &mut tm);
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), cfmt.as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_normalizes() {
        assert_eq!(absolute(Path::new("/a/b/../c/./d")).unwrap(), PathBuf::from("/a/c/d"));
        assert_eq!(absolute(Path::new("/..")).unwrap(), PathBuf::from("/"));
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512B");
        assert_eq!(human_size(1536), "1.5K");
        assert_eq!(human_size(50 * 1024 * 1024), "50M");
    }
}
