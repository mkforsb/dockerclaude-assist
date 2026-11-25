//! build-image, start-container, stop-container, rm-container.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::docker::{self, Container, LABEL_DOT_CLAUDE, LABEL_SPACE};
use crate::paths;
use crate::space::{container_name, network_name};
use crate::ui::{self, Plan};
use crate::{herdr, mounts, tmpfs};

pub fn build_image(tag: Option<&str>, yes: bool) -> Result<()> {
    let tag = tag.unwrap_or("latest");
    let image_ref = docker::image_ref(tag);
    let context = paths::config_dir()?;
    let dockerfile = paths::dockerfile(tag)?;
    let seed = paths::missing_assets(tag)?;
    if !dockerfile.exists() && !seed.iter().any(|a| a.rel == "Dockerfile") {
        bail!(
            "{} does not exist (only the plain Dockerfile, for tag `latest`, has a built-in default)",
            dockerfile.display()
        );
    }
    let old = docker::image(&image_ref)?;
    let old_users = match &old {
        Some(img) => docker::containers_using_image(&img.id)?,
        None => Vec::new(),
    };

    let mut plan = Plan::new(format!("Build image {}", ui::bold(&image_ref)))
        .row("dockerfile", paths::display(&dockerfile))
        .row("context", paths::display(&context));
    if !seed.is_empty() {
        let files: Vec<String> = seed.iter().map(|a| paths::display(&context.join(a.rel))).collect();
        plan = plan.row("write defaults", files.join("\n"));
    }
    plan = match &old {
        Some(img) => {
            let fate = if old_users.is_empty() {
                "removed after the build".to_string()
            } else {
                format!("kept while containers use it: {}", old_users.join(", "))
            };
            plan.row(
                "replaces",
                format!("{} (created {}), {fate}", img.short_id(), docker::image_created_since(&image_ref)),
            )
        }
        None => plan.row("replaces", "nothing (no image yet)"),
    };
    plan.note("Claude Code and hc are downloaded fresh; the layers before them come from cache")
        .note("Existing containers keep running on their old image until recreated")
        .confirm(yes)?;

    if !seed.is_empty() {
        paths::write_assets(&seed)?;
        for a in &seed {
            ui::ok(format!("wrote {}", paths::display(&context.join(a.rel))));
        }
    }

    ui::step(format!("Building {image_ref}"));
    let build_id =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut c = docker::cmd(["build", "-f"]);
    c.arg(&dockerfile)
        .args(["--build-arg", &format!("DOCKERCLAUDE_BUILD_ID={build_id}"), "-t", &image_ref])
        .arg(&context);
    docker::run(c)?;

    let new = docker::image(&image_ref)?.context("the image is missing after the build")?;
    ui::ok(format!("built {image_ref} ({})", new.short_id()));
    if let Some(old) = old
        && old.id != new.id
    {
        if old_users.is_empty() {
            match docker::output(docker::cmd(["image", "rm", &old.id])) {
                Ok(_) => ui::ok(format!("removed the previous image {}", old.short_id())),
                Err(e) => ui::warn(format!("could not remove the previous image {}: {e:#}", old.short_id())),
            }
        } else {
            ui::note(format!(
                "kept the previous image {} for {}; it goes once they are removed (docker image prune)",
                old.short_id(),
                old_users.join(", ")
            ));
        }
    }
    Ok(())
}

/// The host's time zone name, passed to the container as TZ.
fn host_tz() -> Option<String> {
    if let Ok(tz) = std::env::var("TZ")
        && !tz.is_empty()
    {
        return Some(tz.trim_start_matches(':').to_string());
    }
    if let Ok(s) = std::fs::read_to_string("/etc/timezone")
        && let Some(l) = s.lines().next().filter(|l| !l.trim().is_empty())
    {
        return Some(l.trim().to_string());
    }
    let link = std::fs::read_link("/etc/localtime").ok()?;
    let s = link.to_string_lossy();
    s.find("zoneinfo/").map(|i| s[i + "zoneinfo/".len()..].to_string())
}

fn tmpfs_note(size: &str) -> String {
    format!(
        "writable layer moved onto a {size} tmpfs (lost on reboot or docker restart, back to its state at the move)"
    )
}

/// What a new container is made of.
pub struct NewContainer<'a> {
    pub space: &'a str,
    pub image_ref: String,
    pub dot_claude: PathBuf,
    pub tmpfs: Option<&'a str>,
}

/// Creates and starts a container, after confirmation.
pub fn create_container(spec: &NewContainer, title: String, yes: bool) -> Result<()> {
    let name = container_name(spec.space);
    let image = docker::image(&spec.image_ref)?.with_context(|| {
        format!("there is no image {} (build it with `dockerclaude build-image`)", spec.image_ref)
    })?;
    let tz = host_tz();
    let env: Vec<String> = docker::TERMINAL_ENV
        .iter()
        .filter_map(|v| std::env::var(v).ok().map(|val| format!("{v}={val}")))
        .chain(tz.iter().map(|t| format!("TZ={t}")))
        .collect();
    let dot_exists = spec.dot_claude.is_dir();
    let network = network_name(spec.space);
    let network_exists = docker::network_exists(&network)?;

    Plan::new(title)
        .row("container", ui::bold(&name))
        .row(
            "image",
            format!(
                "{} ({}, created {})",
                spec.image_ref,
                image.short_id(),
                docker::image_created_since(&spec.image_ref)
            ),
        )
        .row(
            "dot-claude",
            format!(
                "{} → /workspace/.claude{}",
                paths::display(&spec.dot_claude),
                if dot_exists { "" } else { " (will be created)" }
            ),
        )
        .row("env", if env.is_empty() { "-".into() } else { env.join("\n") })
        .row("network", format!("{network}{}", if network_exists { "" } else { " (will be created)" }))
        .row("options", "security-opt no-new-privileges, restart unless-stopped")
        .row("tmpfs", spec.tmpfs.map(tmpfs_note).unwrap_or_else(|| "no (writable layer on disk)".into()))
        .confirm(yes)?;

    if !dot_exists {
        std::fs::create_dir_all(&spec.dot_claude)
            .with_context(|| format!("creating {}", spec.dot_claude.display()))?;
    }
    let dot = spec.dot_claude.to_str().context("the dot-claude path is not valid UTF-8")?;

    if !network_exists {
        ui::step(format!("Creating network {network}"));
        docker::network_create(&network, spec.space)?;
    }
    // Removes the network again if this call created it and the container
    // could not be made.
    let undo_network = || {
        if !network_exists {
            let _ = remove_network_if_unused(&network);
        }
    };

    ui::step(format!("Creating {name}"));
    let mut c = docker::cmd(["create", "--name", &name, "--network", &network]);
    c.args(["--label", &format!("{LABEL_SPACE}={}", spec.space)])
        .args(["--label", &format!("{LABEL_DOT_CLAUDE}={dot}")])
        .args(["--restart", "unless-stopped", "--security-opt", "no-new-privileges"])
        .args([
            "--mount",
            &format!("type=bind,\"source={}\",target=/workspace/.claude", dot.replace('"', "\"\"")),
        ])
        .args(docker::terminal_env_args());
    if let Some(t) = &tz {
        c.args(["-e", &format!("TZ={t}")]);
    }
    c.arg(&spec.image_ref);
    if let Err(e) = docker::output(c) {
        undo_network();
        return Err(e);
    }

    if let Some(size) = spec.tmpfs {
        ui::step(format!("Moving the writable layer of {name} onto a {size} tmpfs"));
        if let Err(e) = tmpfs::move_to_tmpfs(&name, size) {
            let _ = docker::output(docker::cmd(["rm", &name]));
            undo_network();
            return Err(
                e.context(format!("moving {name} onto a tmpfs failed; removed the new container again"))
            );
        }
    }

    ui::step(format!("Starting {name}"));
    docker::output(docker::cmd(["start", &name]))?;
    ui::ok(format!("{name} is running"));
    Ok(())
}

fn same_path(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

pub fn start_container(
    space: &str,
    tag: Option<&str>,
    dot_claude: Option<&Path>,
    tmpfs_size: Option<&str>,
    yes: bool,
) -> Result<()> {
    let name = container_name(space);
    if let Some(sz) = tmpfs_size {
        tmpfs::parse_size(sz)?;
    }
    let dot_claude = dot_claude.map(paths::absolute).transpose()?;

    match docker::container(&name)? {
        Some(c) if c.running() => {
            let mut wrong = Vec::new();
            if let Some(t) = tag
                && c.image_tag() != Some(t)
            {
                wrong.push(format!("it runs image {}, not {}", c.image, docker::image_ref(t)));
            }
            if let Some(d) = &dot_claude {
                match c.dot_claude() {
                    Some(have) if same_path(&have, d) => {}
                    Some(have) => wrong.push(format!(
                        "its dot-claude is {}, not {}",
                        paths::display(&have),
                        paths::display(d)
                    )),
                    None => wrong.push("it has no dot-claude mount".into()),
                }
            }
            if let Some(sz) = tmpfs_size {
                match tmpfs::current(&name)? {
                    None => wrong.push("its writable layer is not on a tmpfs".into()),
                    Some(t) if !tmpfs::size_matches(sz, t.size)? => {
                        wrong.push(format!("its tmpfs is {}, not {sz}", paths::human_size(t.size)))
                    }
                    Some(_) => {}
                }
            }
            if !wrong.is_empty() {
                bail!(
                    "{name} is already running, but {}.\nTo recreate it: dockerclaude rm-container -s {space}",
                    wrong.join(", ")
                );
            }
            ui::ok(format!("{name} is already running"));
            Ok(())
        }
        Some(c) if c.status == "exited" || c.status == "created" => {
            start_existing(&c, space, tag.is_some() || dot_claude.is_some(), tmpfs_size, yes)
        }
        Some(c) => bail!(
            "container {name} is {}; wait for it, or remove it with `dockerclaude rm-container`",
            c.status
        ),
        None => {
            let spec = NewContainer {
                space,
                image_ref: docker::image_ref(tag.unwrap_or("latest")),
                dot_claude: match dot_claude {
                    Some(d) => d,
                    None => paths::default_dot_claude()?,
                },
                tmpfs: tmpfs_size,
            };
            create_container(&spec, format!("Create and start container {}", ui::bold(&name)), yes)
        }
    }
}

fn start_existing(
    c: &Container,
    space: &str,
    other_args: bool,
    tmpfs_size: Option<&str>,
    yes: bool,
) -> Result<()> {
    let name = &c.name;
    if other_args {
        bail!(
            "container {name} already exists (stopped), and only --space, --tmpfs and --yes apply to an existing \
             container.\nTo recreate it with other settings: dockerclaude rm-container -s {space}"
        );
    }
    let current = match tmpfs_size {
        Some(_) => tmpfs::current(name)?,
        None => tmpfs::current(name).ok().flatten(),
    };
    let move_to = match (tmpfs_size, current) {
        (Some(sz), Some(t)) => {
            if !tmpfs::size_matches(sz, t.size)? {
                bail!("{name} is already on a {} tmpfs, not {sz}", paths::human_size(t.size));
            }
            None
        }
        (Some(sz), None) => Some(sz),
        (None, _) => None,
    };
    let tmpfs_row = match (move_to, current) {
        (Some(sz), _) => tmpfs_note(sz),
        (None, Some(t)) => {
            format!("on a {} tmpfs ({} used)", paths::human_size(t.size), paths::human_size(t.used))
        }
        (None, None) => "no (writable layer on disk)".into(),
    };

    Plan::new(format!("Start existing container {}", ui::bold(name)))
        .row("status", docker::ps_status(name))
        .row("image", format!("{} ({})", c.image, docker::short_id(&c.image_id)))
        .row("dot-claude", c.dot_claude().map(|p| paths::display(&p)).unwrap_or_else(|| "-".into()))
        .row("tmpfs", tmpfs_row)
        .confirm(yes)?;

    if let Some(sz) = move_to {
        ui::step(format!("Moving the writable layer of {name} onto a {sz} tmpfs"));
        tmpfs::move_to_tmpfs(name, sz)?;
    }
    ui::step(format!("Starting {name}"));
    docker::output(docker::cmd(["start", name]))?;
    ui::ok(format!("{name} is running"));
    Ok(())
}

pub fn stop_container(space: &str, yes: bool) -> Result<()> {
    let name = container_name(space);
    let c = docker::container(&name)?.with_context(|| format!("there is no container {name}"))?;
    if !c.running() {
        ui::ok(format!("{name} is already stopped ({})", c.status));
        return Ok(());
    }
    let mounted = mounts::list(&name, c.pid).unwrap_or_default();
    let machine = herdr::hc_available() && herdr::machine_exists(&name).unwrap_or(false);

    Plan::new(format!("Stop container {} (it is kept)", ui::bold(&name)))
        .row("status", docker::ps_status(&name))
        .row("image", &c.image)
        .row(
            "mounts",
            if mounted.is_empty() {
                "-".into()
            } else {
                mounted.iter().map(|m| paths::display(&m.host)).collect::<Vec<_>>().join("\n")
            },
        )
        .note("Everything running in it ends, including claude sessions")
        .note("Its mounts under /mnt go away; `dockerclaude` mounts the current directory again next time")
        .note(if machine { format!("Its Herdr terminals (machine {name}) end too") } else { String::new() })
        .confirm(yes)?;

    ui::step(format!("Stopping {name}"));
    docker::output(docker::cmd(["stop", &name]))?;
    ui::ok(format!("{name} is stopped"));
    Ok(())
}

pub fn rm_container(space: &str, yes: bool) -> Result<()> {
    let name = container_name(space);
    let c = docker::container(&name)?.with_context(|| format!("there is no container {name}"))?;
    let on_tmpfs = tmpfs::current(&name).ok().flatten();
    let machine = herdr::hc_available() && herdr::machine_exists(&name).unwrap_or(false);
    let snapshot = paths::snapshot_file(space)?;
    let network = network_name(space);
    let network_users: Option<Vec<String>> = if docker::network_exists(&network)? {
        Some(docker::network_containers(&network)?.into_iter().filter(|n| n != &name).collect())
    } else {
        None
    };

    Plan::new(format!("Remove container {}", ui::bold(&name)))
        .row("status", docker::ps_status(&name))
        .row("image", format!("{} ({})", c.image, docker::short_id(&c.image_id)))
        .row(
            "dot-claude",
            format!("{} (kept)", c.dot_claude().map(|p| paths::display(&p)).unwrap_or_else(|| "-".into())),
        )
        .row_if(on_tmpfs.is_some(), "tmpfs", "unmounted, its contents discarded")
        .row_if(machine, "herdr", format!("machine {name} removed from Herdr (hc machine rm)"))
        .row_if(
            network_users.is_some(),
            "network",
            match network_users.as_deref() {
                Some([]) => format!("{network} removed (no other container uses it)"),
                Some(users) => format!("{network} kept, used by {}", users.join(", ")),
                None => String::new(),
            },
        )
        .row_if(snapshot.exists(), "snapshot", format!("{} (kept)", paths::display(&snapshot)))
        .note("Everything installed or changed in the container since it was created is lost")
        .note(if snapshot.exists() {
            String::new()
        } else {
            format!("To keep it, run `dockerclaude snapshot -s {space}` first")
        })
        .confirm(yes)?;

    if c.running() {
        ui::step(format!("Stopping {name}"));
        docker::output(docker::cmd(["stop", &name]))?;
    }
    if on_tmpfs.is_some() {
        ui::step("Unmounting its tmpfs");
        tmpfs::unmount(&name)?;
    }
    ui::step(format!("Removing {name}"));
    docker::output(docker::cmd(["rm", &name]))?;
    ui::ok(format!("removed {name}"));
    if network_users.is_some() {
        match remove_network_if_unused(&network) {
            Ok(true) => ui::ok(format!("removed network {network}")),
            Ok(false) => ui::note(format!("kept network {network}: other containers use it")),
            Err(e) => ui::warn(format!("could not remove network {network}: {e:#}")),
        }
    }
    if machine {
        match herdr::machine_rm(&name) {
            Ok(()) => ui::ok(format!("removed machine {name} from Herdr")),
            Err(e) => ui::warn(format!("could not remove machine {name} from Herdr: {e:#}")),
        }
    }
    Ok(())
}

/// Removes network `name` if no container (running or not) is connected to
/// it; returns whether it did.
fn remove_network_if_unused(name: &str) -> Result<bool> {
    if !docker::network_containers(name)?.is_empty() {
        return Ok(false);
    }
    docker::output(docker::cmd(["network", "rm", name]))?;
    Ok(true)
}
