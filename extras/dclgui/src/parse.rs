//! Parsers for the output of `dockerclaude.sh`. No I/O happens here.

use std::collections::HashMap;

use crate::dc::Invocation;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Instance {
    /// `DCPREFIX` of this instance, empty for the default instance.
    pub prefix: String,
    pub container: Option<Container>,
    pub log: Vec<String>,
    pub sessions: Vec<Session>,
    pub snapshot_bytes: Option<u64>,
    pub last_prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Container {
    pub running: bool,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// `None` for the mounts not associated with any session.
    pub name: Option<String>,
    pub running: bool,
    pub mounts: Vec<Mount>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mount {
    /// The host directory, or a description of the problem if `broken`.
    pub label: String,
    pub slug: String,
    pub refcount: u32,
    pub broken: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub description: String,
    pub fix: Option<Invocation>,
}

/// Character offsets of the columns of interest in the `docker ps` table.
#[derive(Clone, Copy)]
struct Columns {
    status: usize,
    ports: usize,
    size: usize,
}

impl Columns {
    fn parse(header: &str) -> Option<Self> {
        let offset = |name: &str| {
            let byte = header.find(name)?;
            Some(header[..byte].chars().count())
        };

        Some(Self {
            status: offset("STATUS")?,
            ports: offset("PORTS")?,
            size: offset("SIZE")?,
        })
    }

    fn parse_row(&self, row: &str) -> Container {
        // Offsets rather than splitting on whitespace, as PORTS is usually empty
        // and STATUS and SIZE contain spaces. In characters, as docker truncates
        // long cells with `…`.
        let chars: Vec<char> = row.chars().collect();
        let cell = |from: usize, to: usize| -> String {
            chars[from.min(chars.len())..to.min(chars.len())]
                .iter()
                .collect::<String>()
                .trim()
                .to_owned()
        };

        Container {
            running: cell(self.status, self.ports).starts_with("Up"),
            size_bytes: parse_docker_size(&cell(self.size, chars.len())),
        }
    }
}

/// Parses a size column such as `1.5GB (virtual 3.2GB)`, going by the writable
/// layer. Docker uses decimal units here.
fn parse_docker_size(size: &str) -> Option<u64> {
    let size = size.split_whitespace().next()?;
    let split = size.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let (value, unit) = size.split_at(split);

    let factor = match unit {
        "B" => 1e0,
        "kB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return None,
    };

    Some((value.parse::<f64>().ok()? * factor) as u64)
}

/// Parses a line below `Session ...:` in `ps` output.
fn parse_mount(line: &str) -> Option<Mount> {
    let line = line.strip_prefix("  ")?;

    if let Some(problem) = line.strip_prefix("Broken mount! ") {
        let (slug, rest) = problem.split_once(" with refcount ")?;
        let refcount = rest.split_whitespace().next()?.parse().ok()?;

        return Some(Mount {
            label: problem.to_owned(),
            slug: slug.to_owned(),
            refcount,
            broken: true,
        });
    }

    // The slug has no spaces, the path might contain anything.
    let (path, rest) = line.rsplit_once(" -> /mnt/")?;
    let (slug, rest) = rest.split_once(" (refcount ")?;
    let refcount = rest.trim_end_matches(')').split(',').next()?.parse().ok()?;

    Some(Mount {
        label: path.to_owned(),
        slug: slug.to_owned(),
        refcount,
        broken: false,
    })
}

/// Parses `ps -a`. Its blocks are separated by `Prefix:` lines, and the block of
/// the default instance comes first and has none.
pub fn parse_ps(output: &str) -> Vec<Instance> {
    let mut instances = vec![Instance::default()];
    let mut columns = None;
    let mut in_mounts = false;

    for line in output.lines() {
        if let Some(prefix) = line.strip_prefix("Prefix: ") {
            instances.push(Instance {
                prefix: prefix.to_owned(),
                ..Instance::default()
            });
            columns = None;
            in_mounts = false;
            continue;
        }

        let instance = instances.last_mut().expect("starts with an instance");

        if line == "Sessions/mounts:" {
            in_mounts = true;
        } else if in_mounts {
            if line == "Unassociated:" {
                instance.sessions.push(Session {
                    name: None,
                    running: false,
                    mounts: Vec::new(),
                });
            } else if let Some(header) = line
                .strip_prefix("Session ")
                .and_then(|s| s.strip_suffix(':'))
            {
                let (name, running) = match header.strip_suffix(" (not running)") {
                    Some(name) => (name, false),
                    None => (header, true),
                };

                instance.sessions.push(Session {
                    name: Some(name.to_owned()),
                    running,
                    mounts: Vec::new(),
                });
            } else if let (Some(mount), Some(session)) =
                (parse_mount(line), instance.sessions.last_mut())
            {
                session.mounts.push(mount);
            }
        } else if let Some(log) = line.strip_prefix("> ") {
            instance.log.push(log.to_owned());
        } else if line.starts_with("CONTAINER ID") {
            columns = Columns::parse(line);
        } else if let (Some(columns), false) = (columns, line.trim().is_empty()) {
            instance.container = Some(columns.parse_row(line));
        }
    }

    instances
}

/// Parses `snapshots-list` into snapshot sizes by prefix.
pub fn parse_snapshots(output: &str) -> HashMap<String, u64> {
    output
        .lines()
        .filter_map(|line| {
            let (size, prefix) = line.split_once('\t')?;
            Some((prefix.to_owned(), size.parse().ok()?))
        })
        .collect()
}

/// Parses a fix as printed by `doctor`, e.g.
/// `DCPREFIX=work- dockerclaude force-clean-mounts`.
fn parse_fix(fix: &str) -> Option<Invocation> {
    let mut words = fix.split_whitespace().peekable();

    let prefix = words
        .next_if(|word| word.starts_with("DCPREFIX="))
        .map(|word| word.trim_start_matches("DCPREFIX=").to_owned())
        .unwrap_or_default();

    words.next().filter(|word| *word == "dockerclaude")?;

    let args: Vec<String> = words.map(str::to_owned).collect();
    (!args.is_empty()).then_some(Invocation { prefix, args })
}

/// Parses `doctor`, where each problem is `<description>\t<fix>`. Other lines,
/// i.e. `No issues found`, have no tab.
pub fn parse_doctor(output: &str) -> Vec<Issue> {
    output
        .lines()
        .filter_map(|line| {
            let (description, fix) = line.split_once('\t')?;
            Some(Issue {
                description: description.to_owned(),
                fix: parse_fix(fix),
            })
        })
        .collect()
}

/// Formats a size for display, in whole MiB.
pub fn format_mib(bytes: u64) -> String {
    format!("{} MiB", (bytes + (1 << 19)) >> 20)
}

/// Shortens `line` to at most `max_columns` columns, ending it in `…` if needed.
/// Wide characters, e.g. CJK, are assumed to take up two columns.
pub fn truncate(line: &str, max_columns: usize) -> String {
    let width = |c: char| if c >= '\u{2E80}' { 2 } else { 1 };

    if line.chars().map(width).sum::<usize>() <= max_columns {
        return line.to_owned();
    }

    let mut used = 1; // The ellipsis.
    let mut truncated: String = line
        .chars()
        .take_while(|c| {
            used += width(*c);
            used <= max_columns
        })
        .collect();

    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = "\
CONTAINER ID   IMAGE                 COMMAND            CREATED      STATUS                    PORTS     NAMES                 SIZE
0123456789ab   dockerclaude:latest   \"/entrypoint.…\"    2 days ago   Up 2 days                           dockerclaude          1.5GB (virtual 3.2GB)
> Nov 05 12:00:01 Auto-update
> Sessions/mounts:

Sessions/mounts:
Session another session (not running):
  /tmp/sp ace -> /mnt/-tmp-sp-ace (refcount 1, mount:another session)
Session sess1:
  /tmp/foo -> /mnt/-tmp-foo (refcount 2, session:sess1)
  Broken mount! -tmp-gone with refcount 3 is not present in filesystem!
Unassociated:
  /tmp/baz -> /mnt/-tmp-baz (refcount 1)
Broken mount! -tmp-stray not present in refcount database!

Prefix: fras-dbdig-

CONTAINER ID   IMAGE                 COMMAND            CREATED      STATUS                    PORTS     NAMES                 SIZE

Sessions/mounts:

Prefix: work-

CONTAINER ID   IMAGE                 COMMAND            CREATED      STATUS                    PORTS     NAMES                 SIZE
0123456789ab   dockerclaude:latest   \"/entrypoint.sh\"   2 days ago   Exited (0) 3 hours ago              work-dockerclaude     0B (virtual 3.2GB)
> log line

Sessions/mounts:
None

";

    #[test]
    fn ps_finds_all_instances() {
        let prefixes: Vec<_> = parse_ps(PS).into_iter().map(|i| i.prefix).collect();
        assert_eq!(prefixes, ["", "fras-dbdig-", "work-"]);
    }

    #[test]
    fn ps_without_any_output_still_has_the_default_instance() {
        assert_eq!(parse_ps(""), [Instance::default()]);
    }

    #[test]
    fn ps_parses_containers() {
        let instances = parse_ps(PS);

        assert_eq!(
            instances[0].container,
            Some(Container {
                running: true,
                size_bytes: Some(1_500_000_000),
            })
        );
        assert_eq!(instances[1].container, None);
        assert_eq!(
            instances[2].container,
            Some(Container {
                running: false,
                size_bytes: Some(0),
            })
        );
    }

    #[test]
    fn ps_parses_logs() {
        let instances = parse_ps(PS);

        assert_eq!(
            instances[0].log,
            ["Nov 05 12:00:01 Auto-update", "Sessions/mounts:"]
        );
        assert!(instances[1].log.is_empty());
    }

    #[test]
    fn ps_parses_sessions() {
        let sessions = &parse_ps(PS)[0].sessions;

        assert_eq!(sessions.len(), 3);

        assert_eq!(sessions[0].name.as_deref(), Some("another session"));
        assert!(!sessions[0].running);
        assert_eq!(sessions[0].mounts[0].label, "/tmp/sp ace");
        assert_eq!(sessions[0].mounts[0].slug, "-tmp-sp-ace");

        assert_eq!(sessions[1].name.as_deref(), Some("sess1"));
        assert!(sessions[1].running);
        assert_eq!(sessions[1].mounts[0].refcount, 2);
        assert_eq!(
            sessions[1].mounts[1],
            Mount {
                label: "-tmp-gone with refcount 3 is not present in filesystem!".to_owned(),
                slug: "-tmp-gone".to_owned(),
                refcount: 3,
                broken: true,
            }
        );

        assert_eq!(sessions[2].name, None);
        assert_eq!(sessions[2].mounts.len(), 1);
        assert_eq!(sessions[2].mounts[0].refcount, 1);
    }

    #[test]
    fn docker_sizes() {
        assert_eq!(parse_docker_size("12.5kB"), Some(12_500));
        assert_eq!(parse_docker_size("456MB (virtual 1GB)"), Some(456_000_000));
        assert_eq!(parse_docker_size("N/A"), None);
        assert_eq!(parse_docker_size(""), None);
    }

    #[test]
    fn snapshots() {
        let snapshots = parse_snapshots("123\t\n5242880\twork-\n");

        assert_eq!(snapshots[""], 123);
        assert_eq!(snapshots["work-"], 5242880);
    }

    #[test]
    fn doctor() {
        let issues = parse_doctor(
            "No dockerclaude image in Docker image store\tdockerclaude build-image\n\
             Cannot reach the Docker daemon\t-\n\
             work-mounts/-a exists but is not mounted\tDCPREFIX=work- dockerclaude force-clean-mounts -a\n",
        );

        assert_eq!(issues.len(), 3);
        assert_eq!(issues[0].fix, Some(Invocation::new("", ["build-image"])));
        assert_eq!(issues[1].fix, None);
        assert_eq!(
            issues[2].fix,
            Some(Invocation::new("work-", ["force-clean-mounts", "-a"]))
        );
    }

    #[test]
    fn doctor_without_issues() {
        assert!(parse_doctor("No issues found\n").is_empty());
    }

    #[test]
    fn mib() {
        assert_eq!(format_mib(5 * 1024 * 1024), "5 MiB");
        assert_eq!(format_mib(1_500_000_000), "1431 MiB");
        assert_eq!(format_mib(0), "0 MiB");
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello!", 5), "hell…");
        assert_eq!(truncate("日本語です", 6), "日本…");
        assert_eq!(truncate("hello", 0), "…");
    }
}
