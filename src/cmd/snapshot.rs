//! snapshot, snapshot-delete, restore.

use std::process::Stdio;

use anyhow::{Context, Result, bail};

use super::container::{NewContainer, create_container};
use crate::docker::{self, LABEL_DOT_CLAUDE};
use crate::paths;
use crate::space::container_name;
use crate::tmpfs;
use crate::ui::{self, Plan};

pub fn snapshot_image(space: &str) -> String {
    docker::image_ref(&format!("snapshot-{space}"))
}

fn file_summary(path: &std::path::Path) -> String {
    match std::fs::metadata(path) {
        Ok(m) => format!(
            "{} ({}, {})",
            paths::display(path),
            paths::human_size(m.len()),
            m.modified().map(paths::local_time).unwrap_or_default()
        ),
        Err(_) => paths::display(path),
    }
}

fn require_zstd() -> Result<()> {
    if crate::herdr::which("zstd").is_none() {
        bail!("zstd is needed for snapshots but is not installed");
    }
    Ok(())
}

pub fn snapshot(space: &str, yes: bool) -> Result<()> {
    let name = container_name(space);
    let c = docker::container(&name)?.with_context(|| format!("there is no container {name}"))?;
    require_zstd()?;
    let file = paths::snapshot_file(space)?;
    let image = snapshot_image(space);

    Plan::new(format!("Snapshot container {}", ui::bold(&name)))
        .row("status", docker::ps_status(&name))
        .row("commit to", &image)
        .row("save to", paths::display(&file))
        .row_if(file.exists(), "replaces", file_summary(&file))
        .note("Includes everything installed in the container, not its dot-claude or mounts")
        .note(if c.running() { "The container is paused while it is committed" } else { "" })
        .confirm(yes)?;

    ui::step(format!("Committing {name} to {image}"));
    docker::output(docker::cmd(["commit", &name, &image]))?;

    ui::step(format!("Saving {image} to {}", paths::display(&file)));
    std::fs::create_dir_all(file.parent().unwrap())?;
    let tmp = file.with_extension("zst.tmp");
    let res = (|| -> Result<()> {
        let mut save =
            docker::cmd(["save", &image]).stdout(Stdio::piped()).spawn().context("running docker save")?;
        let zstd = std::process::Command::new("zstd")
            .args(["-T0", "-q", "-f", "-o"])
            .arg(&tmp)
            .stdin(save.stdout.take().unwrap())
            .status()
            .context("running zstd")?;
        let saved = save.wait()?;
        if !saved.success() || !zstd.success() {
            bail!("docker save | zstd failed");
        }
        Ok(())
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, &file)?;
    ui::ok(format!("saved {}", file_summary(&file)));
    Ok(())
}

pub fn snapshot_delete(space: &str, yes: bool) -> Result<()> {
    let file = paths::snapshot_file(space)?;
    let image_ref = snapshot_image(space);
    let image = docker::image(&image_ref)?;
    if !file.exists() && image.is_none() {
        bail!(
            "there is no snapshot of space {space} ({} and image {image_ref} don't exist)",
            paths::display(&file)
        );
    }
    let users = match &image {
        Some(i) => docker::containers_using_image(&i.id)?,
        None => Vec::new(),
    };

    Plan::new(format!("Delete the snapshot of space {}", ui::bold(space)))
        .row_if(file.exists(), "file", file_summary(&file))
        .row_if(
            image.is_some(),
            "image",
            if users.is_empty() {
                image_ref.clone()
            } else {
                format!("{image_ref} (untagged only; used by {})", users.join(", "))
            },
        )
        .confirm(yes)?;

    if file.exists() {
        std::fs::remove_file(&file).with_context(|| format!("removing {}", file.display()))?;
        ui::ok(format!("deleted {}", paths::display(&file)));
    }
    if image.is_some() {
        // A used image is only untagged (-f), so that containers keep working.
        let args: &[&str] = if users.is_empty() { &["image", "rm"] } else { &["image", "rm", "-f"] };
        let mut c = docker::cmd(args);
        c.arg(&image_ref);
        match docker::output(c) {
            Ok(_) => ui::ok(format!("removed image {image_ref}")),
            Err(e) => ui::warn(format!("kept image {image_ref}: {e:#}")),
        }
    }
    Ok(())
}

pub fn restore(space: &str, tmpfs_size: Option<&str>, yes: bool) -> Result<()> {
    let name = container_name(space);
    let file = paths::snapshot_file(space)?;
    if !file.exists() {
        bail!("there is no snapshot at {}", paths::display(&file));
    }
    if let Some(c) = docker::container(&name)? {
        bail!(
            "container {name} already exists ({}); remove it first with `dockerclaude rm-container -s {space}`",
            c.status
        );
    }
    if let Some(sz) = tmpfs_size {
        tmpfs::parse_size(sz)?;
    }
    require_zstd()?;
    let image_ref = snapshot_image(space);

    Plan::new(format!("Restore container {} from its snapshot", ui::bold(&name)))
        .row("file", file_summary(&file))
        .row("load as", &image_ref)
        .row("dot-claude", "as when the snapshot was taken (the default if unknown)")
        .row(
            "tmpfs",
            tmpfs_size.map(|s| format!("writable layer on a {s} tmpfs")).unwrap_or_else(|| "no".into()),
        )
        .row("options", "security-opt no-new-privileges, restart unless-stopped")
        .confirm(yes)?;

    ui::step(format!("Loading {}", paths::display(&file)));
    let mut zstd = std::process::Command::new("zstd")
        .arg("-dc")
        .arg(&file)
        .stdout(Stdio::piped())
        .spawn()
        .context("running zstd")?;
    let loaded =
        docker::cmd(["load"]).stdin(zstd.stdout.take().unwrap()).status().context("running docker load")?;
    if !zstd.wait()?.success() || !loaded.success() {
        bail!("zstd -dc | docker load failed");
    }

    let image =
        docker::image(&image_ref)?.with_context(|| format!("the snapshot did not contain {image_ref}"))?;
    let dot_claude = match image.labels.get(LABEL_DOT_CLAUDE) {
        Some(d) if !d.is_empty() => d.into(),
        _ => paths::default_dot_claude()?,
    };
    let spec = NewContainer { space, image_ref, dot_claude, tmpfs: tmpfs_size };
    create_container(&spec, String::new(), true)
}
