#!/bin/bash
set -euo pipefail

DC2DIR="$(dirname "$(realpath "$0")")"
DC2MOUNTSDIR="$DC2DIR/mounts"
DC2MOUNTSDB="$DC2DIR/mounts.sqlite3"

function slugify {
    echo "$1" | sed -E 's/[^A-Za-z0-9]/-/g'
}

function bound_source {
    local boundSourceDevice boundFsRoot rootMountPoint hostPath

    read -r boundSourceDevice boundFsRoot < <(findmnt -rn -o MAJ:MIN,FSROOT --mountpoint "$1")
    read -r rootMountPoint < <(findmnt -rn -o MAJ:MIN,FSROOT,TARGET \
        | awk -v dev="${boundSourceDevice}" '$1 == dev && $2 == "/" { print $3; exit }')

    # findmnt -r escapes special characters, e.g. a space becomes \x20
    printf -v hostPath '%b' "${rootMountPoint%/}${boundFsRoot}"

    [ "$(slugify "${hostPath}")" == "$2" ] && echo "${hostPath}"
}

function perform_install {
    if [ -f "$DC2DIR/installers/${1}.sh" ]; then
        docker compose -f "${DC2DIR}/docker-compose.yml" exec -it dockerclaude \
            bash -c "umask 002 && /installers/${1}.sh"
    else
        docker compose -f "${DC2DIR}/docker-compose.yml" exec -it dockerclaude \
            bash -c "umask 002 && apt update && apt install -y $*"
    fi
}

# Runs SQL (the last argument) against the mounts database, passing any other
# arguments as sqlite3 options. Waits for concurrent sessions to release locks.
function mounts_db {
    sqlite3 -cmd '.timeout 10000' "${@:1:$#-1}" "${DC2MOUNTSDB}" "${!#}"
}

# Prints $1 as an SQL string literal.
function sql_quote {
    printf "'%s'" "${1//\'/\'\'}"
}

# Creates the mounts table, adding the session column to databases created
# before sessions existed. session is `session:<name>` for a session's own
# directory, `mount:<name>` for a directory mounted into session <name>, or
# null for a directory not tied to any session.
function init_mounts_db {
    mounts_db " \
        create table if not exists mounts (slug text primary key, reference_count int, session text); \
    "

    if [ "$(mounts_db \
        "select count(*) from pragma_table_info('mounts') where name = 'session';")" == "0" ]; then
        # Tolerate a concurrent session having just added it.
        mounts_db "alter table mounts add column session text;" 2>/dev/null || true
    fi
}

# Prints the reference count for slug $1, or nothing if it has no references.
function mount_refcount {
    mounts_db " \
        select reference_count from mounts \
        where slug = '${1}'; \
    "
}

# Prints the host directory bind-mounted at ./mounts/<slug $1>, falling back
# to the slug itself if it cannot be determined.
function mount_source {
    bound_source "${DC2MOUNTSDIR}/${1}" "${1}" || echo "${1}"
}

# Unmounts ./mounts/<slug $1> and removes its mount point.
function unmount_slug {
    local mountPoint="$DC2MOUNTSDIR/${1}"

    if mountpoint -q "${mountPoint}"; then
        # Only remove the directory once the unmount has actually succeeded;
        # otherwise the rm would reach through the bind mount into the real
        # project directory.
        if sudo umount "${mountPoint}"; then
            rmdir "${mountPoint}"
        else
            echo "dockerclaude: failed to unmount ${mountPoint}, leaving it in place" >&2
            return 1
        fi
    fi
}

# Bind-mounts the absolute host directory $1 under ./mounts (unless it is
# already mounted) and takes a reference on it on behalf of session $2 (e.g.
# `session:foo` or `mount:foo`, or empty for none). Fails if the directory is
# already mounted on behalf of another session.
function acquire_mount {
    local slug mountPoint session current
    slug=$(slugify "$1")
    mountPoint="$DC2MOUNTSDIR/${slug}"
    session=NULL
    [ -n "${2:-}" ] && session=$(sql_quote "$2")

    init_mounts_db

    current=$(mounts_db " \
        select ifnull('for ' || session, 'without a session') from mounts \
        where slug = '${slug}' and session is not ${session}; \
    ")
    if [ -n "${current}" ]; then
        echo "dockerclaude: $1 is already mounted ${current}" >&2
        return 1
    fi

    if ! mountpoint -q "${mountPoint}"; then
        mkdir -p "${mountPoint}"
        sudo mount --bind "$1" "${mountPoint}"
    fi

    mounts_db " \
        insert into mounts (slug, reference_count, session)  \
        values ('${slug}', 1, ${session}) \
        on conflict(slug) do update set reference_count = reference_count + 1; \
    "
}

# Drops a reference on the absolute host directory $1 and unmounts it once no
# references remain. If that was the last mount of session `session:<name>`,
# every directory mounted with `mount:<name>` is unmounted as well.
function release_mount {
    local slug session name mountSession s rc=0
    local -a sessionMounts=()
    slug=$(slugify "$1")

    init_mounts_db

    session=$(mounts_db "select session from mounts where slug = '${slug}';")

    mounts_db " \
        update mounts set reference_count = reference_count - 1 \
        where slug = '${slug}'; \
        \
        delete from mounts where reference_count <= 0; \
    "

    if [ "$(mount_refcount "${slug}")" == "" ]; then
        unmount_slug "${slug}" || rc=1
    fi

    if [[ "${session}" == session:* ]]; then
        name="${session#session:}"
        mountSession=$(sql_quote "mount:${name}")

        mapfile -t sessionMounts < <(mounts_db " \
            begin immediate; \
            select slug from mounts where session = ${mountSession} \
                and not exists (select 1 from mounts where session = $(sql_quote "${session}")); \
            delete from mounts where session = ${mountSession} \
                and not exists (select 1 from mounts where session = $(sql_quote "${session}")); \
            commit; \
        ")

        for s in "${sessionMounts[@]}"; do
            echo "Session ${name} ended, unmounting $(mount_source "${s}") from /mnt/${s}"
            unmount_slug "${s}" || rc=1
        done
    fi

    return "${rc}"
}

# Parses `<path> [<session-name>]` for subcommand $1 into the globals
# targetPath and sessionName (empty if not given).
function parse_mount_args {
    local cmd="$1"
    shift

    case $# in
        1) targetPath="$1"; sessionName="" ;;
        2) targetPath="$1"; sessionName="$2" ;;
        *)
            echo "usage: dockerclaude.sh ${cmd} <path> [<session-name>]" >&2
            exit 1
            ;;
    esac

    if [ ! -d "${targetPath}" ]; then
        echo "Invalid path \`${targetPath}\`" >&2
        exit 1
    fi
}

# Starts a session in directory $1 (default: the current directory), named $2
# (default: the directory's absolute path).
function start_session {
    local targetDir
    targetDir=$(realpath "${1:-.}")

    acquire_mount "${targetDir}" "session:${2:-${targetDir}}"

    # From here on, always release the mount/refcount on exit, including when
    # the session is interrupted or the exec fails.
    trap "release_mount $(printf '%q' "${targetDir}")" EXIT

    docker compose -f "${DC2DIR}/docker-compose.yml" exec -it dockerclaude \
        bash -c "umask 002 && cd /mnt/$(slugify "${targetDir}") && claude"
}

# Renames session $1 to $2, i.e. moves all mounts of `session:$1` and
# `mount:$1` to `session:$2` and `mount:$2`.
function rename_session {
    local renamed

    init_mounts_db

    renamed=$(mounts_db " \
        update mounts set session = $(sql_quote "session:${2}") \
        where session = $(sql_quote "session:${1}"); \
        update mounts set session = $(sql_quote "mount:${2}") \
        where session = $(sql_quote "mount:${1}"); \
        select total_changes(); \
    ")

    if [ "${renamed}" == "0" ]; then
        echo "No session named ${1}" >&2
        return 1
    fi

    echo "Renamed session ${1} to ${2} (${renamed} mount(s))"
}

# Wipes ./dot-claude except for credentials, config and skills, after stashing a
# compressed copy of the session history in ./session-history.
function dot_claude_clean {
    local claudeDir="${DC2DIR}/dot-claude"
    local historyDir="${DC2DIR}/session-history"
    local ans f archive
    local -a toArchive=()

    read -r -p "Really wipe ${claudeDir}? Y/n? " -n 1 ans </dev/tty
    echo

    if [ ! "${ans}" == "Y" ]; then
        echo "You declined the cleanup"
        return 0
    fi

    for f in file-history history.jsonl plans projects session-env sessions shell-snapshots; do
        if [ -e "${claudeDir}/${f}" ]; then
            toArchive+=("dot-claude/${f}")
        fi
    done

    # set -e aborts before anything is removed if the archive can't be written.
    if [ "${#toArchive[@]}" -gt 0 ]; then
        mkdir -p "${historyDir}"
        archive="${historyDir}/$(date +%Y%m%d-%H%M%S).tar.gz"
        tar -C "${DC2DIR}" -czf "${archive}" "${toArchive[@]}"
        echo "Saved session history to ${archive}"
    fi

    find "${claudeDir}" -mindepth 1 -maxdepth 1 \
        ! -name ".credentials.json" \
        ! -name ".claude.json" \
        ! -name "settings.json" \
        ! -name "skills" \
        ! -name ".gitkeep" \
        -exec rm -vrf {} +
}

if [ "${1:-}" == "install" ]; then
    shift
    perform_install "$@"
elif [ "${1:-}" == "exec" ]; then
    shift
    docker compose -f "${DC2DIR}/docker-compose.yml" exec -it dockerclaude bash -c 'umask 002 && exec "$@"' bash "$@"
elif [ "${1:-}" == "make" ]; then
    shift
    cd "${DC2DIR}" && make "$@"
elif [ "${1:-}" == "dir" ]; then
    shift
    echo "${DC2DIR}"
elif [ "${1:-}" == "dot-claude-clean" ]; then
    dot_claude_clean
elif [ "${1:-}" == "mount" ]; then
    shift
    parse_mount_args mount "$@"

    targetDir=$(realpath "${targetPath}")
    slug=$(slugify "${targetDir}")
    acquire_mount "${targetDir}" "${sessionName:+mount:${sessionName}}"
    echo "${targetDir} -> dockerclaude:/mnt/${slug} (refcount $(mount_refcount "${slug}"))"
elif [ "${1:-}" == "umount" ]; then
    shift
    if [ $# -ne 1 ]; then
        echo "usage: dockerclaude.sh umount <path>" >&2
        exit 1
    fi

    # -m: allow unmounting even if the source directory has since been removed
    targetDir=$(realpath -m "$1")
    slug=$(slugify "${targetDir}")
    init_mounts_db

    if [ -z "$(mount_refcount "${slug}")" ] && ! mountpoint -q "${DC2MOUNTSDIR}/${slug}"; then
        echo "${targetDir} is not mounted" >&2
        exit 1
    fi

    release_mount "${targetDir}"

    refCount=$(mount_refcount "${slug}")
    if [ -n "${refCount}" ]; then
        echo "${targetDir} is still in use (refcount ${refCount})"
    else
        echo "Unmounted ${targetDir} from dockerclaude:/mnt/${slug}"
    fi
elif [ "${1:-}" == "rename-session" ]; then
    shift
    if [ $# -ne 2 ] || [ -z "$1" ] || [ -z "$2" ]; then
        echo "usage: dockerclaude.sh rename-session <old-name> <new-name>" >&2
        exit 1
    fi

    rename_session "$1" "$2"
elif [ "${1:-}" == "ps" ]; then
    docker ps -as -f name=dockerclaude

    (IFS=$'\n' && for s in $(docker logs dockerclaude | tail -n2); do echo "> $s"; done)

    echo
    echo "Sessions/mounts:"
    # ./mounts must be a shared mount point (see `make setup-mounts`), otherwise
    # project mounts will not propagate into the running container.
    if ! mountpoint -q "${DC2MOUNTSDIR}" || \
        [[ "$(findmnt -n -o PROPAGATION --mountpoint "${DC2MOUNTSDIR}")" != *shared* ]]; then
        echo "${DC2MOUNTSDIR} is not mounted properly! (run 'make setup-mounts')" >&2
        exit 1
    fi

    mapfile -t mountsOnDisk < <(
        find "${DC2MOUNTSDIR}" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort
    )

    # Bring databases from before sessions existed up to date, without creating
    # one if it is missing.
    [ -f "${DC2MOUNTSDB}" ] && init_mounts_db

    # One row per mount: session name, `session`/`mount` (both empty if not
    # associated with a session), slug and refcount. Grouped by session with
    # the session's own mounts first and unassociated mounts last.
    # A missing or unreadable database is treated as empty.
    mapfile -t mountsRefCount < <(
        mounts_db -readonly -separator $'\x1f' " \
            select ifnull(substr(session, instr(session, ':') + 1), ''), \
                   ifnull(substr(session, 1, instr(session, ':') - 1), ''), \
                   slug, reference_count \
            from mounts \
            order by session is null, 1, 2 desc, 3;" 2>/dev/null
    )

    if [ "${#mountsOnDisk[@]}" -eq 0 ] && [ "${#mountsRefCount[@]}" -eq 0 ]; then
        echo 'None'
        exit 0
    fi

    declare -A onDisk=() inDb=()
    for slug in "${mountsOnDisk[@]}"; do
        onDisk["${slug}"]=1
    done

    group=$'\n'
    for row in "${mountsRefCount[@]}"; do
        IFS=$'\x1f' read -r name kind slug refcount <<< "${row}"
        inDb["${slug}"]=1

        if [ "${kind:+session:}${name}" != "${group}" ]; then
            group="${kind:+session:}${name}"
            if [ -z "${kind}" ]; then
                echo "Unassociated:"
            elif [ "${kind}" == "mount" ]; then
                # Sessions sort before mounts, so there is no session:<name>.
                echo "Session ${name} (not running):"
            else
                echo "Session ${name}:"
            fi
        fi

        if [ -z "${onDisk[${slug}]:-}" ]; then
            echo "  Broken mount! ${slug} with refcount ${refcount} is not present in filesystem!"
        elif ! mountpoint -q "${DC2MOUNTSDIR}/${slug}"; then
            echo "  Broken mount! ${slug} with refcount ${refcount} exists but is not mounted!"
        else
            echo "  $(mount_source "${slug}") -> /mnt/${slug} (refcount ${refcount}${kind:+, ${kind}:${name}})"
        fi
    done

    for slug in "${mountsOnDisk[@]}"; do
        if [ -z "${inDb[${slug}]:-}" ]; then
            echo "Broken mount! ${slug} not present in refcount database!"
        fi
    done
elif [ $# -le 2 ] && [ -d "${1:-.}" ]; then
    start_session "${1:-.}" "${2:-}"
else
    echo "Invalid path or command \`$*\`" >&2
    echo "usage: dockerclaude.sh [<path> [<session-name>]]" >&2
    exit 1
fi
