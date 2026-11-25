//! bash-completion and its `__list` helper.

use anyhow::Result;

use crate::docker::{self, CONTAINER_PREFIX};
use crate::mounts;
use crate::paths;
use crate::space::{self, Unnamed, container_name};

const SCRIPT: &str = r#"# dockerclaude bash completion; in ~/.bashrc: source <(dockerclaude bash-completion)
_dockerclaude_list() {
    dockerclaude __list "$@" 2>/dev/null
}

# Adds the words on stdin (one per line) that start with $cur to COMPREPLY,
# shell-quoted, so that paths with spaces work.
_dockerclaude_add() {
    local w q
    while IFS= read -r w; do
        printf -v q '%q' "$w"
        [[ "$q" == "$cur"* || "$w" == "$cur"* ]] && COMPREPLY+=("$q")
    done
}

_dockerclaude() {
    local cur="${COMP_WORDS[COMP_CWORD]}" prev="${COMP_WORDS[COMP_CWORD-1]}"
    local sub="" space="" npos=0 i w
    COMPREPLY=()

    for ((i = 1; i < COMP_CWORD; i++)); do
        w="${COMP_WORDS[i]}"
        case "$w" in
            -s|--space) space="${COMP_WORDS[i+1]}"; ((i++)) ;;
            --space=*) space="${w#--space=}" ;;
            -t|--image-tag|-d|--dot-claude|--tmpfs) ((i++)) ;;
            -*) ;;
            *) if [ -z "$sub" ]; then sub="$w"; else ((npos++)); fi ;;
        esac
    done

    case "$prev" in
        -s|--space)
            _dockerclaude_add < <(_dockerclaude_list spaces)
            return ;;
        -t|--image-tag)
            _dockerclaude_add < <(_dockerclaude_list tags)
            return ;;
        -d|--dot-claude)
            compopt -o filenames
            mapfile -t COMPREPLY < <(compgen -d -- "$cur")
            return ;;
        --tmpfs)
            return ;;
    esac

    # exec: after the command word, complete like any command line.
    if [ "$sub" == "exec" ] && [ "$npos" -ge 1 ]; then
        compopt -o default
        return
    fi

    local flags="" words=""
    case "$sub" in
        "")
            flags="-s --space --streaming-json -y --yes -h --help -V --version"
            words="build-image start-container stop-container rm-container install exec dir dot-claude-clean snapshot snapshot-delete restore mount umount ps bash-completion" ;;
        build-image) flags="-t --image-tag -y --yes" ;;
        start-container) flags="-s --space -t --image-tag -d --dot-claude --tmpfs -y --yes" ;;
        stop-container|rm-container|snapshot|snapshot-delete) flags="-s --space -y --yes" ;;
        restore) flags="-s --space --tmpfs -y --yes" ;;
        install) flags="-s --space -y --yes" ;;
        exec) flags="-s --space -y --yes" ;;
        dot-claude-clean) flags="-d --dot-claude -y --yes" ;;
        mount) flags="-s --space --ro -y --yes" ;;
        umount) flags="-s --space -y --yes" ;;
        ps) flags="-s --space -a --all" ;;
        *) return ;;
    esac

    if [[ "$cur" == -* ]]; then
        mapfile -t COMPREPLY < <(compgen -W "$flags" -- "$cur")
        return
    fi

    case "$sub" in
        "")
            mapfile -t COMPREPLY < <(compgen -W "$words" -- "$cur") ;;
        install)
            _dockerclaude_add < <(_dockerclaude_list installers) ;;
        exec)
            mapfile -t COMPREPLY < <(compgen -c -- "$cur" | sort -u) ;;
        mount)
            [ "$npos" -eq 0 ] || return
            compopt -o filenames
            mapfile -t COMPREPLY < <(compgen -d -- "$cur") ;;
        umount)
            [ "$npos" -eq 0 ] || return
            if [ -n "$space" ]; then
                _dockerclaude_add < <(_dockerclaude_list mounts --space "$space")
            else
                _dockerclaude_add < <(_dockerclaude_list mounts)
            fi ;;
    esac
}
complete -F _dockerclaude dockerclaude
"#;

pub fn bash_completion() -> Result<i32> {
    print!("{SCRIPT}");
    Ok(0)
}

pub fn list(what: &str, space: Option<String>) -> Result<i32> {
    let items: Vec<String> = match what {
        "spaces" => {
            let mut v: Vec<String> = docker::dockerclaude_containers()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|n| n.strip_prefix(CONTAINER_PREFIX).map(String::from))
                .collect();
            if let Ok(entries) =
                paths::config_dir().map(|d| d.join("snapshots")).and_then(|d| Ok(std::fs::read_dir(d)?))
            {
                v.extend(
                    entries
                        .filter_map(|e| e.ok()?.file_name().into_string().ok())
                        .filter_map(|n| n.strip_suffix(".tar.zst").map(String::from)),
                );
            }
            v.sort();
            v.dedup();
            v
        }
        "mounts" => {
            let space = match space {
                Some(s) => s,
                None => space::resolve(None, Unnamed::Note)?.space,
            };
            let name = container_name(&space);
            match docker::container(&name)? {
                Some(c) if c.running() => mounts::list(&name, c.pid)?
                    .into_iter()
                    .map(|m| m.host.to_string_lossy().into_owned())
                    .collect(),
                _ => Vec::new(),
            }
        }
        "installers" => paths::installer_names(),
        "tags" => paths::dockerfile_tags(),
        _ => Vec::new(),
    };
    for i in items {
        println!("{i}");
    }
    Ok(0)
}
