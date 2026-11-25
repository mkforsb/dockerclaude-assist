//! Spaces: one container (`dockerclaude-<space>`) each, and in Herdr one hc
//! machine of the same name.

use anyhow::{Result, bail};

use crate::docker::CONTAINER_PREFIX;
use crate::herdr::{self, Pane};
use crate::ui::{self, Plan};

pub const DEFAULT: &str = "default";

/// hc machine names are 1-32 of `a-z 0-9 - _ .`, and the machine is named
/// like the container: `dockerclaude-` (13) + the space.
pub const MAX_LEN: usize = 32 - CONTAINER_PREFIX.len();

pub fn validate(s: &str) -> Result<String, String> {
    let ok_start = s.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let ok_chars = s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.".contains(c));
    if s.is_empty() || s.len() > MAX_LEN || !ok_start || !ok_chars {
        return Err(format!(
            "invalid space name `{s}`: use 1-{MAX_LEN} of a-z 0-9 - _ . starting with a letter or digit"
        ));
    }
    Ok(s.to_string())
}

/// Turns a Herdr space name into a space name: lowercase, runs of anything
/// else than a-z 0-9 _ . become `-`, trimmed and shortened to fit.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed: String =
        out.trim_matches(|c| c == '-' || c == '.' || c == '_').chars().take(MAX_LEN).collect();
    trimmed.trim_end_matches(['-', '.', '_']).to_string()
}

pub fn container_name(space: &str) -> String {
    format!("{CONTAINER_PREFIX}{space}")
}

/// The space's user-defined Docker network, named like its container. Unlike
/// the default bridge, it gives the container Docker's embedded DNS server,
/// which also reaches resolvers that only the host can reach.
pub fn network_name(space: &str) -> String {
    container_name(space)
}

/// The space a command works on, and the Herdr pane it runs in, if any.
#[derive(Debug, Clone)]
pub struct Target {
    pub space: String,
    pub pane: Option<Pane>,
}

impl Target {
    pub fn container(&self) -> String {
        container_name(&self.space)
    }
}

/// How to treat an unnamed Herdr space.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Unnamed {
    /// Use "default", with a note.
    Note,
    /// Use "default", after asking (unless --yes), with advice on naming it.
    Confirm { yes: bool },
}

/// `--space` if given, else the named Herdr space we run in, else "default".
pub fn resolve(explicit: Option<String>, unnamed: Unnamed) -> Result<Target> {
    let pane = herdr::detect();
    if let Some(space) = explicit {
        return Ok(Target { space, pane });
    }
    let Some(p) = &pane else {
        return Ok(Target { space: DEFAULT.into(), pane });
    };

    let slug = slugify(&p.space_name);
    if !p.space_name.is_empty() && validate(&slug).is_ok() {
        if slug != p.space_name {
            ui::note(format!("Herdr space `{}` → dockerclaude space `{slug}`", p.space_name));
        }
        return Ok(Target { space: slug, pane });
    }

    let why = if p.space_name.is_empty() {
        "This Herdr space has no name".to_string()
    } else {
        format!("The Herdr space name `{}` has no usable letters or digits", p.space_name)
    };
    match unnamed {
        Unnamed::Note => {
            ui::note(format!("{why}; using space `{DEFAULT}`"));
        }
        Unnamed::Confirm { yes } => {
            let suggestion = p
                .space_folder
                .as_deref()
                .and_then(|f| f.rsplit('/').find(|s| !s.is_empty()))
                .map(slugify)
                .filter(|s| validate(s).is_ok())
                .unwrap_or_else(|| "my-space".into());
            let rename = serde_json::json!({"id": p.space_id, "name": suggestion}).to_string();
            Plan::new(format!("{why}, so this session would go to dockerclaude space `{DEFAULT}`"))
                .row("herdr space", &p.space_id)
                .row_if(p.space_folder.is_some(), "folder", p.space_folder.as_deref().unwrap_or(""))
                .row("container", container_name(DEFAULT))
                .note("A named Herdr space gets a dockerclaude space (and container) of the same name.")
                .note("To name this Herdr space (it keeps the name from then on), run:")
                .note(format!("  herdr-app do space.rename {}", ui::shell_quote(&rename)))
                .note("Or pick a space explicitly: dockerclaude --space <space>")
                .confirm(yes)?;
        }
    }
    Ok(Target { space: DEFAULT.into(), pane })
}

/// For commands that only make sense with an existing container.
pub fn require_running(c: &Option<crate::docker::Container>, name: &str) -> Result<crate::docker::Container> {
    match c {
        None => bail!("there is no container {name} (create it with `dockerclaude start-container`)"),
        Some(c) if !c.running() => {
            bail!("container {name} is {} (start it with `dockerclaude start-container`)", c.status)
        }
        Some(c) => Ok(c.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slugify("My Project"), "my-project");
        assert_eq!(slugify("  --foo__bar.. "), "foo__bar");
        assert_eq!(slugify("Ünïcode ✓ stuff"), "n-code-stuff");
        assert_eq!(slugify("日本"), "");
        assert_eq!(slugify("a".repeat(40).as_str()).len(), MAX_LEN);
    }

    #[test]
    fn validation() {
        assert!(validate("default").is_ok());
        assert!(validate("a-b_c.d").is_ok());
        assert!(validate("Foo").is_err());
        assert!(validate("-foo").is_err());
        assert!(validate("").is_err());
        assert!(validate(&"a".repeat(MAX_LEN + 1)).is_err());
        assert_eq!(container_name("x").len() + MAX_LEN - 1, 32);
    }
}
