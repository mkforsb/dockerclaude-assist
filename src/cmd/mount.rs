//! mount, umount.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::docker;
use crate::mounts::{self, Visibility};
use crate::paths;
use crate::space::{container_name, require_running};
use crate::ui::{self, Plan};

pub fn mount(space: &str, path: &Path, readonly: bool, yes: bool) -> Result<()> {
    let name = container_name(space);
    let c = require_running(&docker::container(&name)?, &name)?;
    let host = std::fs::canonicalize(paths::absolute(path)?).with_context(|| path.display().to_string())?;
    if host == Path::new("/") {
        bail!("refusing to mount the whole filesystem");
    }
    let target = mounts::target_for(&host);
    let current = mounts::list(&name, c.pid)?;

    match mounts::visibility(&host, &current) {
        Visibility::Mounted(m) => {
            if readonly && !m.readonly {
                ui::warn(format!("{} is already mounted, read-write", paths::display(&host)));
            }
            ui::ok(format!(
                "{} is already mounted in {name} at {}",
                paths::display(&host),
                m.target.display()
            ));
            Ok(())
        }
        Visibility::Inside(m) => {
            ui::ok(format!(
                "{} is already visible in {name} at {}, through the mount of {}",
                paths::display(&host),
                target.display(),
                paths::display(&m.host)
            ));
            Ok(())
        }
        Visibility::Covers(inner) => bail!(
            "{} contains directories that are mounted already:\n  {}\nUnmount those first (dockerclaude umount), \
             then mount {} instead",
            paths::display(&host),
            inner.iter().map(|m| paths::display(&m.host)).collect::<Vec<_>>().join("\n  "),
            paths::display(&host)
        ),
        Visibility::Absent => {
            Plan::new(format!("Mount into container {}", ui::bold(&name)))
                .row("host", host.display())
                .row("container", target.display())
                .row("access", if readonly { "read-only" } else { "read-write" })
                .note(if readonly {
                    ""
                } else {
                    "The container can then change and delete files there, as you"
                })
                .note("Mounts last until the container stops")
                .confirm(yes)?;
            mounts::mount(c.pid, &host, readonly)?;
            ui::ok(format!("mounted {} at {name}:{}", paths::display(&host), target.display()));
            Ok(())
        }
    }
}

pub fn umount(space: &str, path: &Path, yes: bool) -> Result<()> {
    let name = container_name(space);
    let c = require_running(&docker::container(&name)?, &name)?;
    let current = mounts::list(&name, c.pid)?;
    // As given (it may be gone on the host by now), or with symlinks resolved.
    let given = paths::absolute(path)?;
    let resolved = paths::resolve(path)?;
    let Some(m) = current.iter().find(|m| m.host == given || m.host == resolved).cloned() else {
        let mut msg = format!("{} is not mounted in {name}", paths::display(&given));
        if let Visibility::Inside(parent) = mounts::visibility(&resolved, &current) {
            msg.push_str(&format!(" (it is visible through the mount of {})", paths::display(&parent.host)));
        } else if !current.is_empty() {
            msg.push_str(&format!(
                "; mounted are:\n  {}",
                current.iter().map(|m| paths::display(&m.host)).collect::<Vec<_>>().join("\n  ")
            ));
        }
        bail!(msg);
    };
    let busy = mounts::processes_in(&name, &m.target);

    let mut plan = Plan::new(format!("Unmount from container {}", ui::bold(&name)))
        .row("host", m.host.display())
        .row("container", m.target.display());
    if !busy.is_empty() {
        plan = plan
            .row("in use by", busy.join("\n"))
            .note("These processes are working in it; they lose access to its files (lazy unmount)");
    }
    plan.note("The host files are not touched; empty mount point directories are removed").confirm(yes)?;

    mounts::umount(c.pid, &m)?;
    ui::ok(format!("unmounted {} from {name}", paths::display(&m.host)));
    Ok(())
}
